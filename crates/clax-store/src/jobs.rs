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
use super::docs::{BatchWrite, Written};
use super::questions::Reason;
use clax_core::audit::AuditCtx;
use clax_core::db::{Caller, Level};
use clax_core::feedback::Touched;
use clax_core::model::{Artifact, Comment, Version};
use clax_core::publish::ValidatedPublish;
use clax_core::{ArtifactId, CoreError};
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
    /// The tool call that asked it (`questions.tool_use_id`): the app's
    /// queue names a job's `ask` call `<job>/turn:<n>/<call ID>`, so a worker
    /// resumed after a kill knows which call each question answers.
    pub tool_use_id: Option<String>,
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
            "SELECT jq.question_id, jq.outcome, jq.answers_json, q.tool_use_id
             FROM job_questions jq JOIN questions q ON q.id = jq.question_id
             WHERE jq.job_id = ?1 ORDER BY jq.question_id",
        )?
        .query_map(params![job.id], |r| {
            Ok(JobQuestion {
                question_id: r.get(0)?,
                tool_use_id: r.get(3)?,
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
    /// transaction. Every step's commit clears the job's `error`: an error
    /// a job keeps while it runs again names what refused the step before
    /// (spec 2026-10-19-in-process-core-design §7.4), and a committed step
    /// is past it. A key the job already has returns its step as recorded
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
                "UPDATE jobs SET step = ?2, state = ?3, error_json = NULL WHERE id = ?1",
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
    /// row). The agent's read consents naming `aid` (as reader or source)
    /// and its draft's renders go too: their foreign keys cascade only on a
    /// hard delete, and a soft-deleted artifact is never undeleted. Called
    /// by every delete of an artifact, the soft delete included, before it.
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
        tx.execute(
            "DELETE FROM agent_read_consents WHERE reader = ?1 OR source = ?1",
            params![aid],
        )?;
        tx.execute(
            "DELETE FROM draft_renders WHERE draft_id = ?1",
            params![aid],
        )?;
        Ok(())
    }

    /// Forgets thread `tid` in every job that answers it, inside the
    /// caller's transaction, which is deleting it: each such job that is
    /// not final moves one `rev` on. Called by every delete of a thread,
    /// before it.
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
    /// go, and each job that had one moves one `rev` on, unless it is
    /// final: no write changes a `cancelled` or `done` job's row.
    fn threads_going(&self, tx: &Transaction<'_>, which: &str, key: &str) -> clax_core::Result<()> {
        tx.execute(
            &format!(
                "UPDATE jobs SET rev = rev + 1, updated_at = ?2
                 WHERE id IN (SELECT job_id FROM job_threads WHERE {which})
                   AND state NOT IN ('cancelled', 'done')"
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

/// A draft whose render a publish step names (`POST /api/drafts/{id}/renders`
/// records them): the draft and the digest of its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftRef {
    pub id: String,
    pub digest: String,
}

/// The mark publish step `key` of job `id` writes its version's content
/// with ([`clax_core::content::VersionMeta::step`]): the two and `nonce`,
/// the random value the store minted for the step when it first wrote
/// (`job_step_marks`), as a JSON array, one line. The job ID and key say
/// whose it is to a reader of the content; only the nonce, which the store
/// keeps, makes it the step's.
pub fn step_mark(id: &str, key: &str, nonce: &str) -> String {
    serde_json::to_string(&[id, key, nonce]).expect("three strings serialise")
}

/// What a step mark found on content no row records is, as the store
/// tells it from the marks it holds (`job_step_marks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkOf {
    /// The mark of the step writing now.
    Own,
    /// The mark of another step that may still commit: its job is open and
    /// the key is not committed.
    Live,
    /// The mark of a step that can never commit: its job is gone or final,
    /// or the key is committed.
    Dead,
    /// No mark the store holds for that artifact and version: a mark
    /// copied or guessed into content (a pushed commit's message), or one
    /// written before marks were the store's.
    Foreign,
}

/// What `mark`, found on version `n` of artifact `aid` with no row
/// recording it, is ([`MarkOf`]); `own` is the mark of the step writing,
/// if one is.
pub(crate) fn mark_of(
    c: &Connection,
    aid: &str,
    n: u32,
    mark: &str,
    own: Option<&str>,
) -> rusqlite::Result<MarkOf> {
    let Ok([_, _, nonce]) = serde_json::from_str::<[String; 3]>(mark) else {
        return Ok(MarkOf::Foreign);
    };
    let held: Option<(String, String)> = c
        .query_row(
            "SELECT job_id, key FROM job_step_marks
             WHERE nonce = ?1 AND artifact_id = ?2 AND version_n = ?3",
            params![nonce, aid, n],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((job, key)) = held else {
        return Ok(MarkOf::Foreign);
    };
    if step_mark(&job, &key, &nonce) != mark {
        return Ok(MarkOf::Foreign);
    }
    let live: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE id = ?1 AND state NOT IN ('cancelled', 'done'))
            AND NOT EXISTS (SELECT 1 FROM job_steps WHERE job_id = ?1 AND key = ?2)",
        params![job, key],
        |r| r.get(0),
    )?;
    Ok(match (live, own == Some(mark)) {
        (true, true) => MarkOf::Own,
        (true, false) => MarkOf::Live,
        (false, _) => MarkOf::Dead,
    })
}

/// A publish step's body: the version it publishes and the document writes
/// that land with it.
pub struct PublishStep {
    /// The artifact it publishes a version of; `None` makes a new one.
    pub artifact_id: Option<String>,
    /// The version the step was planned on: the artifact's current version
    /// (0 for a new one).
    pub base: u32,
    pub request: ValidatedPublish,
    /// Document writes, each pinned to the version it was planned on
    /// (or last-writer-wins only where the write says `if_version` is
    /// absent and the document does not exist).
    pub docs: Vec<BatchWrite>,
    /// The draft the files were rendered from.
    pub draft: Option<DraftRef>,
    /// The agent session the version is recorded as published from.
    pub session_id: Option<String>,
    /// The step is the job's last: the job ends `done` with it.
    pub done: bool,
}

/// A reply step's body: the agent's replies, resolves and acknowledgements
/// for a change. The replies come from the session that published the
/// job's version (the version its last publish step made records it), which
/// the store looks up: the worker that resumes a change after a relaunch
/// runs under a new session of its own, and the feedback the reply answers
/// is the publishing session's.
pub struct ReplyStep {
    /// The threads it replies on, each a thread of the job's artifact.
    pub threads: Vec<String>,
    pub body: String,
    /// Each thread is resolved as well.
    pub resolve: bool,
    /// Comments whose feedback the session acknowledges.
    pub ack: Vec<String>,
    /// The step is the job's last: the job ends `done` with it.
    pub done: bool,
}

/// A committed step and the job after it; `replayed` when the key was
/// already committed, and nothing changed.
#[derive(Debug)]
pub struct StepCommit<T> {
    pub step: StepRow,
    pub job: JobRow,
    pub replayed: bool,
    /// What the step wrote, for the events that follow its commit; `None`
    /// on a replay.
    pub made: Option<T>,
}

/// What a publish step wrote.
#[derive(Debug)]
pub struct Published {
    pub artifact: Artifact,
    pub version: Version,
    pub docs: Vec<Written>,
}

/// What a reply step wrote.
#[derive(Debug, Default)]
pub struct Replied {
    /// The session the replies came from: the one that published the job's
    /// version.
    pub session_id: String,
    /// The agent's comments, one per thread, in the step's order.
    pub comments: Vec<Comment>,
    /// The feedback rows the replies, resolves and acknowledgements changed.
    pub touched: Touched,
}

/// Why a step was refused; nothing of it was recorded.
#[derive(Debug)]
pub enum StepError {
    /// The artifact is no longer at the step's base.
    VersionMoved {
        current: u32,
    },
    /// A pinned document write found the document at another version.
    DocMoved {
        path: String,
    },
    /// The draft named has no clean render of the step's files.
    NotRenderedClean,
    Job(JobError),
    Store(CoreError),
}

impl From<JobError> for StepError {
    fn from(e: JobError) -> Self {
        match e {
            JobError::Store(e) => StepError::Store(e),
            e => StepError::Job(e),
        }
    }
}

impl From<CoreError> for StepError {
    fn from(e: CoreError) -> Self {
        StepError::Store(e)
    }
}

impl From<rusqlite::Error> for StepError {
    fn from(e: rusqlite::Error) -> Self {
        StepError::Store(e.into())
    }
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepError::VersionMoved { current } => {
                write!(f, "the artifact moved to version {current}")
            }
            StepError::DocMoved { path } => write!(f, "document {path} moved"),
            StepError::NotRenderedClean => {
                f.write_str("the draft has no clean render of these files")
            }
            StepError::Job(e) => e.fmt(f),
            StepError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StepError {}

type StepResult<T> = std::result::Result<StepCommit<T>, StepError>;

/// Inside a step's transaction: a refusal, kept while the transaction rolls
/// back, or a race lost to a commit of the same key.
enum Cut {
    Refused(StepError),
    Replayed,
}

/// `e` as a step's refusal: a pinned document write that found its
/// document moved is [`StepError::DocMoved`].
fn doc_refusal(e: CoreError) -> StepError {
    match e {
        CoreError::InBatch { path, error, .. }
            if matches!(
                *error,
                CoreError::DocConflict { .. } | CoreError::DocPinRequired { .. }
            ) =>
        {
            StepError::DocMoved { path }
        }
        e => StepError::Store(e),
    }
}

impl Store {
    /// Commits publish step `key` of job `id` with the version `s`
    /// publishes, under `ctx`: the version's content is written first, as
    /// [`Store::publish_version`] writes it, and then one transaction
    /// checks the artifact is at `s.base`, records the version (or the new
    /// artifact), applies `s.docs`, inserts the step with the version's
    /// number, and moves the job one `rev` on with `step` set to `key` (and
    /// `artifact_id` to a new artifact's), ending it `done` when `s.done`.
    /// Either the step and the version exist together, or neither does. A
    /// key the job already has answers its step as recorded, replayed, and
    /// writes nothing.
    ///
    /// # Errors
    /// [`StepError::VersionMoved`]; [`StepError::DocMoved`];
    /// [`StepError::Job`] for a missing or final job; the publish's own
    /// errors.
    pub fn commit_publish_step(
        &self,
        ctx: &AuditCtx,
        id: &str,
        key: &str,
        s: PublishStep,
    ) -> StepResult<Published> {
        self.block_on(async { Ok(self.commit_publish_step_async(ctx, id, key, s).await) })
            .unwrap_or_else(|e| Err(e.into()))
    }

    /// [`Store::commit_publish_step`], awaiting its content store.
    pub async fn commit_publish_step_async(
        &self,
        ctx: &AuditCtx,
        id: &str,
        key: &str,
        s: PublishStep,
    ) -> StepResult<Published> {
        self.publish_step_cut(ctx, id, key, s, |_| Ok(())).await
    }

    /// [`Store::commit_publish_step_async`] with `cut` run last in the
    /// step's transaction, before it commits: an error from it rolls the
    /// version and the step back together.
    async fn publish_step_cut(
        &self,
        ctx: &AuditCtx,
        id: &str,
        key: &str,
        s: PublishStep,
        cut: impl FnOnce(&Transaction<'_>) -> clax_core::Result<()>,
    ) -> StepResult<Published> {
        if let Some(done) = self.replay(id, key)? {
            return Ok(done);
        }
        let PublishStep {
            artifact_id,
            base,
            mut request,
            docs,
            session_id,
            done,
            ..
        } = s;
        let named = artifact_id.as_deref().map(ArtifactId::parse).transpose()?;
        if named.is_none() && base != 0 {
            return Err(StepError::Store(CoreError::invalid(
                "invalid_base",
                "a step that makes a new artifact has base 0",
            )));
        }
        let begun =
            self.begin_publish_step(id, key, named.as_ref(), base + 1, session_id.as_deref())?;
        request.step = Some(step_mark(id, key, &begun.nonce));
        let aid = begun.artifact_id;
        let mut cutting = None;
        let then = |tx: &Transaction<'_>, n: u32| {
            let made = publish_step_tx(self, tx, ctx, id, key, &aid, n, docs, done);
            match made.and_then(|m| match cut(tx) {
                Ok(()) => Ok(m),
                Err(e) => Err(Cut::Refused(e.into())),
            }) {
                Ok(m) => Ok(m),
                Err(c) => {
                    cutting = Some(c);
                    // Stands in for the cut, kept above, to roll back.
                    Err(CoreError::TaskFailed)
                }
            }
        };
        let out = match named {
            None => {
                self.create_artifact_then_async(ctx, &aid, request, session_id.as_deref(), then)
                    .await
            }
            Some(_) => {
                request.if_version = Some(base);
                self.publish_version_then_async(ctx, &aid, request, session_id.as_deref(), then)
                    .await
            }
        };
        match (out, cutting) {
            (Ok((artifact, version, (step, job, docs))), _) => Ok(StepCommit {
                step,
                job,
                replayed: false,
                made: Some(Published {
                    artifact,
                    version,
                    docs,
                }),
            }),
            (Err(_), Some(Cut::Refused(e))) => Err(e),
            (Err(_), Some(Cut::Replayed)) => self
                .replay(id, key)?
                .ok_or(StepError::Job(JobError::NotFound)),
            // Refused before the step's transaction: another call of the
            // same key may have committed it meanwhile (a retry overlapping
            // the first call), which moved the artifact past `base`.
            (Err(e), None) => match self.replay(id, key)? {
                Some(done) => Ok(done),
                None => Err(match e {
                    CoreError::Conflict { current } => StepError::VersionMoved { current },
                    e => StepError::Store(e),
                }),
            },
        }
    }

    /// The mark of publish step `key` of job `id`, which writes version `n`
    /// of `named` (`None`: a new app, whose ID is minted with the mark and
    /// kept, so every attempt of the step writes the same artifact and a
    /// resume replaces what a killed attempt left), and the artifact it
    /// writes: the ones the store keeps for the step (`job_step_marks`),
    /// minted at its first write. A step that now
    /// writes another artifact or version is given a new nonce, so content
    /// an earlier attempt left under the old one is no longer its own.
    /// `session` is recorded as the session it publishes from.
    fn begin_publish_step(
        &self,
        id: &str,
        key: &str,
        named: Option<&ArtifactId>,
        n: u32,
        session: Option<&str>,
    ) -> Result<Begun, StepError> {
        self.with_tx(|tx| {
            let held: Option<(String, String, u32)> = tx
                .query_row(
                    "SELECT nonce, artifact_id, version_n FROM job_step_marks
                     WHERE job_id = ?1 AND key = ?2",
                    params![id, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let now = self.now();
            let aid = match (&held, named) {
                (_, Some(named)) => named.as_str().to_string(),
                (Some((_, aid, _)), None) => aid.clone(),
                (None, None) => ArtifactId::generate().as_str().to_string(),
            };
            let nonce = match held {
                Some((nonce, held_aid, held_n)) if held_aid == aid && held_n == n => {
                    tx.execute(
                        "UPDATE job_step_marks SET session_id = ?2 WHERE nonce = ?1",
                        params![nonce, session],
                    )?;
                    nonce
                }
                _ => {
                    let nonce = new_nonce();
                    tx.execute(
                        "INSERT INTO job_step_marks
                            (nonce, job_id, key, artifact_id, version_n, session_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT (job_id, key) DO UPDATE SET nonce = ?1,
                            artifact_id = ?4, version_n = ?5, session_id = ?6, created_at = ?7",
                        params![nonce, id, key, aid, n, session, now],
                    )?;
                    nonce
                }
            };
            Ok(Begun {
                nonce,
                artifact_id: ArtifactId::parse(&aid)?,
            })
        })
        .map_err(StepError::Store)
    }

    /// Whether `mark`, which version `n` of `aid` carries with no row
    /// recording it, is a mark the store gave one of its steps for that
    /// version ([`MarkOf`] other than [`MarkOf::Foreign`]): content a
    /// step's write left, never a push.
    pub(crate) fn is_step_mark(
        &self,
        aid: &ArtifactId,
        n: u32,
        mark: &str,
    ) -> clax_core::Result<bool> {
        self.with_read(|c| Ok(mark_of(c, aid.as_str(), n, mark, None)? != MarkOf::Foreign))
    }

    /// What `mark`, which version `n` of `aid` carries with no row
    /// recording it, is to the step whose mark is `own` ([`MarkOf`]).
    pub(crate) fn mark_of(
        &self,
        aid: &ArtifactId,
        n: u32,
        mark: &str,
        own: Option<&str>,
    ) -> clax_core::Result<MarkOf> {
        self.with_read(|c| Ok(mark_of(c, aid.as_str(), n, mark, own)?))
    }

    /// The versions job steps wrote that no row records and that still
    /// carry the step's mark (`clax doctor` lists them): each with the job
    /// and key that wrote it, and whether that step may still commit and
    /// replace it (`live`); a dead one is removed by the next write of its
    /// version. By artifact and version.
    ///
    /// # Errors
    /// The store's and the content store's.
    pub fn marked_strays(&self) -> clax_core::Result<Vec<MarkedStray>> {
        self.block_on(self.marked_strays_async())
    }

    /// [`Store::marked_strays`], awaiting its content store.
    pub async fn marked_strays_async(&self) -> clax_core::Result<Vec<MarkedStray>> {
        let held: Vec<(String, String, String, String, u32)> =
            self.with_read(|c| {
                Ok(c.prepare(
                "SELECT m.nonce, m.job_id, m.key, m.artifact_id, m.version_n FROM job_step_marks m
                 WHERE m.version_n > COALESCE(
                     (SELECT current_version FROM artifacts WHERE id = m.artifact_id), 0)
                 ORDER BY m.artifact_id, m.version_n, m.job_id, m.key",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?)
            })?;
        let mut out = Vec::new();
        for (nonce, job_id, key, aid, n) in held {
            let Ok(id) = ArtifactId::parse(&aid) else {
                continue;
            };
            let mark = step_mark(&job_id, &key, &nonce);
            if self.content.unrecorded_step(&id, n).await?.as_deref() != Some(mark.as_str()) {
                continue;
            }
            let live = self.mark_of(&id, n, &mark, None)? == MarkOf::Live;
            out.push(MarkedStray {
                artifact_id: aid,
                n,
                job_id,
                key,
                live,
            });
        }
        Ok(out)
    }

    /// Forgets step mark `mark` once the content it marked is removed.
    pub(crate) fn forget_step_mark(&self, mark: &str) -> clax_core::Result<()> {
        let Ok([job, key, nonce]) = serde_json::from_str::<[String; 3]>(mark) else {
            return Ok(());
        };
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM job_step_marks WHERE nonce = ?1 AND job_id = ?2 AND key = ?3",
                params![nonce, job, key],
            )?;
            Ok(())
        })
    }

    /// Commits reply step `key` of job `id`, under `ctx`, in one
    /// transaction: an agent reply of `s.body`, from the session that
    /// published the version of the job's last publish step
    /// ([`publishing_session_of`]), on each of `s.threads` (each a thread of the job's artifact), with
    /// [`Store::add_agent_reply`]'s ownership rules, each thread then
    /// resolved by that agent when `s.resolve` and its feedback
    /// acknowledged, the session's feedback on comments `s.ack`
    /// acknowledged, the step inserted, and the job moved one `rev` on with
    /// `step` set to `key`, ending it `done` when `s.done`. A key the job
    /// already has answers its step as recorded, replayed, and writes
    /// nothing.
    ///
    /// # Errors
    /// [`StepError::Job`] for a missing or final job; `invalid_step` when
    /// the job has no publish step; `unknown_session` when its version
    /// names no session, or one that ended; `invalid_thread` for a thread
    /// not of the job's artifact; the reply's and the resolve's own errors.
    pub fn commit_reply_step(
        &self,
        ctx: &AuditCtx,
        id: &str,
        key: &str,
        s: ReplyStep,
    ) -> StepResult<Replied> {
        if let Some(done) = self.replay(id, key)? {
            return Ok(done);
        }
        let named = self
            .with_read(|c| publishing_session_of(c, id))?
            .ok_or_else(|| {
                CoreError::invalid(
                    "invalid_step",
                    "a reply step follows the job's publish step",
                )
            })?;
        let sess = named
            .and_then(|sid| self.get_session(&sid).transpose())
            .transpose()?
            .filter(|x| x.ended_at.is_none())
            .ok_or_else(|| {
                CoreError::invalid(
                    "unknown_session",
                    "the job's version was published from no session that is live",
                )
            })?;
        // Read before the transaction, as an agent reply reads them.
        let seen = s
            .threads
            .iter()
            .map(|t| self.seen_comments(t, &sess.id))
            .collect::<clax_core::Result<Vec<_>>>()?;
        let vis = crate::visibility::Visibility::for_session(self, &sess.id)?;
        let mut cutting = None;
        let out = self.with_tx(|tx| {
            match reply_step_tx(self, tx, ctx, id, key, &sess, &s, &seen, &vis) {
                Ok(v) => Ok(v),
                Err(c) => {
                    cutting = Some(c);
                    Err(CoreError::TaskFailed)
                }
            }
        });
        match (out, cutting) {
            (Ok((step, job, made)), _) => Ok(StepCommit {
                step,
                job,
                replayed: false,
                made: Some(made),
            }),
            (Err(_), Some(Cut::Refused(e))) => Err(e),
            (Err(_), Some(Cut::Replayed)) => self
                .replay(id, key)?
                .ok_or(StepError::Job(JobError::NotFound)),
            (Err(e), None) => Err(StepError::Store(e)),
        }
    }

    /// Step `key` of job `id` as recorded, with the job as it is, when the
    /// job has it.
    ///
    /// # Errors
    /// [`JobError::NotFound`] when there is no job `id`; [`JobError::Moved`]
    /// when it has no such step and is final.
    fn replay<T>(&self, id: &str, key: &str) -> Result<Option<StepCommit<T>>, StepError> {
        let (row, step) = self.with_read(|c| Ok((job_row(c, id)?, step_row(c, id, key)?)))?;
        let row = row.ok_or(StepError::Job(JobError::NotFound))?;
        match step {
            Some(step) => Ok(Some(StepCommit {
                step,
                job: row,
                replayed: true,
                made: None,
            })),
            None if row.state.is_final() => Err(StepError::Job(JobError::Moved(Box::new(row)))),
            None => Ok(None),
        }
    }
}

/// The session that published the version of job `id`'s last publish
/// step (`versions.session_id`), read on `c`: `None` when the job has no
/// publish step, `Some(None)` when its version names no session. The
/// reaper keeps it live while the job is open (`OPEN_JOB_SESSIONS`).
fn publishing_session_of(c: &Connection, id: &str) -> clax_core::Result<Option<Option<String>>> {
    Ok(c.prepare_cached(
        "SELECT v.session_id FROM jobs j
           JOIN job_steps s ON s.job_id = j.id AND s.version_n IS NOT NULL
           JOIN versions v ON v.artifact_id = j.artifact_id AND v.n = s.version_n
          WHERE j.id = ?1
          ORDER BY s.rowid DESC LIMIT 1",
    )?
    .query_row(params![id], |r| r.get(0))
    .optional()?)
}

/// A version a job step wrote that no row records, still carrying the
/// step's mark ([`Store::marked_strays`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedStray {
    pub artifact_id: String,
    pub n: u32,
    pub job_id: String,
    pub key: String,
    /// The step may still commit (its job is open and the key is not
    /// committed), and only it may replace the version; else the next
    /// write of the version removes it.
    pub live: bool,
}

/// A publish step's mark and the artifact it writes, as the store keeps
/// them ([`Store::begin_publish_step`]).
struct Begun {
    nonce: String,
    artifact_id: ArtifactId,
}

/// A step's nonce: 32 hex characters from the operating system's random
/// source.
fn new_nonce() -> String {
    clax_core::secret::random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Inserts step `key` of job `id` with `result` (and `version_n`), and
/// moves the job by [`Move::Step`]: its `step` set to `key`, its
/// `artifact_id` to `artifact` when it names none, its `error` cleared
/// (as [`Store::commit_step`] clears it), `done` when `done`, one `rev` on.
/// [`Cut::Replayed`] when the key landed meanwhile.
#[allow(clippy::too_many_arguments)]
fn insert_step(
    st: &Store,
    tx: &Transaction<'_>,
    id: &str,
    key: &str,
    result: &serde_json::Value,
    version_n: Option<u32>,
    artifact: Option<&str>,
    done: bool,
) -> Result<(StepRow, JobRow), Cut> {
    let refused = |e: JobError| Cut::Refused(e.into());
    if step_row(tx, id, key)
        .map_err(|e| refused(e.into()))?
        .is_some()
    {
        return Err(Cut::Replayed);
    }
    let row = existing(tx, id).map_err(refused)?;
    let state = moving(tx, &row, Move::Step { done }).map_err(refused)?;
    let now = st.now();
    let wrote = (|| -> JobResult<(StepRow, JobRow)> {
        tx.execute(
            "INSERT INTO job_steps (job_id, key, result_json, version_n, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, key, text(result), version_n, now],
        )?;
        tx.execute(
            "UPDATE jobs SET step = ?2, state = ?3, artifact_id = COALESCE(artifact_id, ?4),
                             error_json = NULL
             WHERE id = ?1",
            params![id, key, state, artifact],
        )?;
        if state.is_final() {
            st.ending(tx, id)?;
        }
        let row = bump(tx, id, &now)?;
        let step = step_row(tx, id, key)?.ok_or(JobError::NotFound)?;
        Ok((step, row))
    })();
    wrote.map_err(refused)
}

/// The body of a publish step's transaction, once version `n` of its
/// artifact is recorded: the document writes, then the step.
#[allow(clippy::too_many_arguments)]
fn publish_step_tx(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    key: &str,
    aid: &ArtifactId,
    n: u32,
    docs: Vec<BatchWrite>,
    done: bool,
) -> Result<(StepRow, JobRow, Vec<Written>), Cut> {
    let aid_id = aid;
    let aid = aid.as_str().to_string();
    // A change publishes only its own artifact.
    let named: Option<String> = tx
        .query_row(
            "SELECT artifact_id FROM jobs WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| Cut::Refused(e.into()))?
        .flatten();
    if named.as_deref().is_some_and(|a| a != aid) {
        return Err(Cut::Refused(
            CoreError::invalid(
                "invalid_artifact",
                format!("job {id} changes another artifact than {aid}"),
            )
            .into(),
        ));
    }
    let written = if docs.is_empty() {
        Vec::new()
    } else {
        let owner = Caller::new(Level::Owner, None);
        st.doc_batch_in(tx, ctx, aid_id, docs, false, &owner)
            .map_err(|e| Cut::Refused(doc_refusal(e)))?
    };
    let result = serde_json::json!({"artifact_id": aid, "version": n, "docs": written.len()});
    let (step, row) = insert_step(st, tx, id, key, &result, Some(n), Some(&aid), done)?;
    // Committed: its version is recorded, and its mark names nothing left.
    tx.execute(
        "DELETE FROM job_step_marks WHERE job_id = ?1 AND key = ?2",
        params![id, key],
    )
    .map_err(|e| Cut::Refused(e.into()))?;
    Ok((step, row, written))
}

/// The body of a reply step's transaction.
#[allow(clippy::too_many_arguments)]
fn reply_step_tx(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    key: &str,
    sess: &clax_core::model::Session,
    s: &ReplyStep,
    seen: &[Vec<String>],
    vis: &crate::visibility::Visibility,
) -> Result<(StepRow, JobRow, Replied), Cut> {
    let refused = |e: CoreError| Cut::Refused(e.into());
    if step_row(tx, id, key).map_err(refused)?.is_some() {
        return Err(Cut::Replayed);
    }
    let row = existing(tx, id).map_err(|e| Cut::Refused(e.into()))?;
    if row.state.is_final() {
        return Err(Cut::Refused(JobError::Moved(Box::new(row)).into()));
    }
    let aid = row.artifact_id.clone().ok_or_else(|| {
        refused(CoreError::invalid(
            "invalid_thread",
            format!("job {id} names no artifact to reply on"),
        ))
    })?;
    let live: bool = tx
        .query_row(
            "SELECT kind = ?2 FROM artifacts WHERE id = ?1",
            params![aid, clax_core::live::KIND_LIVE],
            |r| r.get(0),
        )
        .map_err(|e| refused(e.into()))?;
    let mut made = Replied {
        session_id: sess.id.clone(),
        ..Replied::default()
    };
    let by = format!("agent:{}", sess.harness);
    let person = sess
        .person
        .as_deref()
        .unwrap_or(clax_core::visibility::OWNER);
    for (tid, seen) in s.threads.iter().zip(seen) {
        let t = super::threads::thread_in(tx, tid).map_err(refused)?;
        if t.is_none_or(|t| t.artifact_id != aid) {
            return Err(refused(CoreError::invalid(
                "invalid_thread",
                format!("{tid} is not a thread of {aid}"),
            )));
        }
        let reply = super::threads::NewComment {
            author_kind: super::threads::AUTHOR_AGENT,
            author_name: sess.harness.clone(),
            author_public_id: None,
            via_session_id: Some(sess.id.clone()),
            body: s.body.clone(),
            via_page: false,
        };
        let (c, _) = st
            .add_agent_reply_in(tx, ctx, tid, reply, seen, &mut made.touched)
            .map_err(refused)?;
        made.comments.push(c);
        if s.resolve {
            let parsed = ArtifactId::parse(&aid).map_err(refused)?;
            let touched = st
                .resolve_thread_in(tx, ctx, tid, &by, Some(person), |tx, _| {
                    if live {
                        super::live::mark_pending_in(
                            tx,
                            &parsed,
                            tid,
                            "resolve",
                            &sess.harness,
                            &st.now(),
                        )?;
                        Ok(None)
                    } else {
                        super::changelog::link_on_resolve_in(tx, tid, &st.now())
                    }
                })
                .map_err(refused)?;
            made.touched.merge(touched);
        }
    }
    st.acknowledge_threads_in(tx, ctx, &sess.id, &s.threads, vis, &mut made.touched)
        .map_err(refused)?;
    st.acknowledge_comments_in(tx, ctx, &sess.id, &s.ack, vis, &mut made.touched)
        .map_err(refused)?;
    let result = serde_json::json!({
        "comments": made.comments.iter().map(|c| serde_json::json!({
            "thread_id": c.thread_id, "comment_id": c.id,
        })).collect::<Vec<_>>(),
        "resolved": s.resolve,
    });
    let (step, row) = insert_step(st, tx, id, key, &result, None, None, s.done)?;
    Ok((step, row, made))
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
        let tool_use_id = (source == Source::Hook).then(|| "toolu_1".to_string());
        question_for(st, hsid, source, tool_use_id)
    }

    /// A question asked by the session with harness session ID `hsid` for
    /// tool call `tool_use_id`.
    fn question_for(st: &Store, hsid: &str, source: Source, tool_use_id: Option<String>) -> String {
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
                tool_use_id,
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

    fn request(html: &str, if_version: Option<u32>) -> ValidatedPublish {
        clax_core::publish::validate(
            serde_json::from_value(json!({
                "title": "Budget",
                "if_version": if_version,
                "files": {"index.html": {"content": html, "encoding": "utf8"}}
            }))
            .unwrap(),
        )
        .unwrap()
    }

    /// A publish step of `html` on `aid` at `base` (a new app for `None`).
    fn publish_step(aid: Option<&ArtifactId>, base: u32, html: &str, done: bool) -> PublishStep {
        PublishStep {
            artifact_id: aid.map(|a| a.as_str().to_string()),
            base,
            request: request(html, None),
            docs: Vec::new(),
            draft: None,
            session_id: None,
            done,
        }
    }

    /// A change of `aid`, created and running.
    fn change(st: &Store, id: &str, aid: &ArtifactId) -> JobRow {
        st.create_job(NewJob {
            kind: "change".into(),
            artifact_id: Some(aid.as_str().into()),
            ..new_job(id)
        })
        .unwrap();
        st.patch_job(
            id,
            JobPatch {
                state: Some(JobState::Running),
                ..patch(1)
            },
        )
        .unwrap()
    }

    fn versions(st: &Store, aid: &ArtifactId) -> Vec<u32> {
        st.list_versions(aid).unwrap().iter().map(|v| v.n).collect()
    }

    #[test]
    fn a_publish_step_makes_its_version_and_ends_the_job_with_it() {
        let (_d, st) = store();
        running(&st, "j1");
        let c = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        assert!(!c.replayed);
        let made = c.made.unwrap();
        assert_eq!(made.version.n, 1);
        assert_eq!(c.step.version_n, Some(1));
        assert_eq!(c.step.result["artifact_id"], json!(made.artifact.id));
        assert_eq!(
            (c.job.state, c.job.step.as_deref(), c.job.rev),
            (JobState::Done, Some("publish:0"), 3)
        );
        assert_eq!(
            c.job.artifact_id.as_deref(),
            Some(made.artifact.id.as_str())
        );
        assert!(st.open_jobs("app").unwrap().is_empty());
    }

    #[test]
    fn a_replayed_publish_step_publishes_once() {
        let (_d, st) = store();
        running(&st, "j1");
        let first = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        let again = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>b", true),
            )
            .unwrap();
        assert!(again.replayed);
        assert!(again.made.is_none());
        assert_eq!(again.step, first.step);
        assert_eq!(again.job, first.job);
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
        assert!(st.zero_version_artifacts().unwrap().is_empty());
    }

    #[test]
    fn a_publish_step_on_a_moved_base_records_nothing() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let before = change(&st, "j1", &aid);
        st.publish_version(DAEMON, &aid, request("<p>v2", Some(1)), None)
            .unwrap();
        let e = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:1",
                publish_step(Some(&aid), 1, "<p>x", false),
            )
            .unwrap_err();
        assert!(matches!(e, StepError::VersionMoved { current: 2 }), "{e:?}");
        assert_eq!(versions(&st, &aid), vec![1, 2]);
        let job = st.job("j1").unwrap().unwrap();
        assert!(job.steps.is_empty());
        assert_eq!(job.job, before);
    }

    #[test]
    fn a_moved_document_refuses_the_whole_step() {
        let (_d, st) = store();
        let aid = crate::test_util::artifact_with_caps(&st, json!({"db": {}}));
        let owner = Caller::new(Level::Owner, None);
        let pin = super::super::docs::Pin {
            if_version: None,
            lww: true,
        };
        let w = st
            .doc_set(DAEMON, &aid, "notes/n1", json!({"t": 1}), pin, &owner)
            .unwrap();
        let planned = w.doc.unwrap().version;
        st.doc_set(DAEMON, &aid, "notes/n1", json!({"t": 2}), pin, &owner)
            .unwrap();
        let before = change(&st, "j1", &aid);
        let mut step = publish_step(Some(&aid), 1, "<p>v2", false);
        step.docs = vec![BatchWrite {
            path: "notes/n1".into(),
            op: super::super::docs::BatchOp::Set(json!({"t": 3})),
            if_version: Some(planned),
        }];
        let e = st
            .commit_publish_step(DAEMON, "j1", "publish:1", step)
            .unwrap_err();
        assert!(
            matches!(&e, StepError::DocMoved { path } if path == "notes/n1"),
            "{e:?}"
        );
        assert_eq!(versions(&st, &aid), vec![1]);
        assert!(!crate::test_util::content_version_exists(&st, &aid, 2));
        assert_eq!(st.job("j1").unwrap().unwrap().job, before);
    }

    #[test]
    fn a_publish_step_lands_its_pinned_documents_with_its_version() {
        let (_d, st) = store();
        let aid = crate::test_util::artifact_with_caps(&st, json!({"db": {}}));
        change(&st, "j1", &aid);
        let mut step = publish_step(Some(&aid), 1, "<p>v2", false);
        step.docs = vec![BatchWrite {
            path: "notes/n1".into(),
            op: super::super::docs::BatchOp::Set(json!({"t": 1})),
            if_version: None,
        }];
        let c = st
            .commit_publish_step(DAEMON, "j1", "publish:1", step)
            .unwrap();
        let made = c.made.unwrap();
        assert_eq!((made.version.n, made.docs.len()), (2, 1));
        assert_eq!(c.step.result["docs"], json!(1));
        assert_eq!(c.job.state, JobState::Running);
    }

    /// Publish step `key` of job `id` with `cut` run last in its
    /// transaction, where a process that dies before the commit stops.
    fn cut_step(
        st: &Store,
        id: &str,
        key: &str,
        s: PublishStep,
        cut: impl FnOnce(&Transaction<'_>) -> clax_core::Result<()>,
    ) -> StepResult<Published> {
        clax_core::content::wait(st.publish_step_cut(DAEMON, id, key, s, cut))
    }

    #[test]
    fn a_step_cut_before_commit_leaves_no_version_and_resumes_once() {
        let (_d, st) = store();
        // A new app: cut after its step is written, before the commit.
        let before = running(&st, "j1");
        let cut = |_: &Transaction<'_>| Err(CoreError::invalid("cut", "the process died"));
        let e = cut_step(
            &st,
            "j1",
            "publish:0",
            publish_step(None, 0, "<p>a", true),
            cut,
        )
        .unwrap_err();
        assert!(
            matches!(e, StepError::Store(CoreError::Invalid { code: "cut", .. })),
            "{e:?}"
        );
        assert!(st.list_artifacts().unwrap().is_empty());
        assert!(st.zero_version_artifacts().unwrap().is_empty());
        let job = st.job("j1").unwrap().unwrap();
        assert!(job.steps.is_empty());
        assert_eq!(job.job, before);
        // The resume commits it once.
        let c = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        assert!(!c.replayed);
        let again = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        assert!(again.replayed);
        assert_eq!(st.list_artifacts().unwrap().len(), 1);

        // A change: the cut leaves version 1 current and no content at 2.
        let aid = artifact(&st, None);
        let before = change(&st, "j2", &aid);
        cut_step(
            &st,
            "j2",
            "publish:1",
            publish_step(Some(&aid), 1, "<p>v2", false),
            cut,
        )
        .unwrap_err();
        assert_eq!(versions(&st, &aid), vec![1]);
        assert!(!crate::test_util::content_version_exists(&st, &aid, 2));
        assert_eq!(st.job("j2").unwrap().unwrap().job, before);
        let c = st
            .commit_publish_step(
                DAEMON,
                "j2",
                "publish:1",
                publish_step(Some(&aid), 1, "<p>v2", false),
            )
            .unwrap();
        assert_eq!(c.step.version_n, Some(2));
        let again = st
            .commit_publish_step(
                DAEMON,
                "j2",
                "publish:1",
                publish_step(Some(&aid), 1, "<p>v2", false),
            )
            .unwrap();
        assert!(again.replayed);
        assert_eq!(versions(&st, &aid), vec![1, 2]);
    }

    #[test]
    fn a_new_apps_step_resumed_over_its_killed_attempts_row_makes_one_artifact() {
        let (_d, st) = store();
        running(&st, "j1");
        let cut = |_: &Transaction<'_>| Err(CoreError::invalid("cut", "the process died"));
        cut_step(
            &st,
            "j1",
            "publish:0",
            publish_step(None, 0, "<p>a", true),
            cut,
        )
        .unwrap_err();
        // A kill leaves the zero-version row its cleanup would have removed.
        let minted: String = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT artifact_id FROM job_step_marks WHERE job_id = 'j1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        st.with_write(|c| {
            Ok(c.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, contract_version)
                 VALUES (?1, 'Budget', 'x', 'x', 0, '0')",
                [&minted],
            )?)
        })
        .unwrap();
        let c = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        assert_eq!(c.made.unwrap().artifact.id, minted);
        assert!(st.zero_version_artifacts().unwrap().is_empty());
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
        // Committed: its mark is gone.
        let marks: i64 = st
            .with_read(
                |c| Ok(c.query_row("SELECT COUNT(*) FROM job_step_marks", [], |r| r.get(0))?),
            )
            .unwrap();
        assert_eq!(marks, 0);
    }

    #[test]
    fn only_a_mark_the_store_holds_for_its_version_is_a_steps() {
        let (_d, st) = store();
        running(&st, "j1");
        let n = "0123456789abcdef0123456789abcdef";
        st.with_write(|c| {
            Ok(c.execute(
                "INSERT INTO job_step_marks (nonce, job_id, key, artifact_id, version_n, created_at)
                 VALUES (?1, 'j1', 'publish:1', 'aaaaaaaaaaaa', 2, 'x')",
                [n],
            )?)
        })
        .unwrap();
        let mark = step_mark("j1", "publish:1", n);
        let of = |aid: &str, v: u32, mark: &str, own: Option<&str>| {
            st.with_read(|c| Ok(mark_of(c, aid, v, mark, own)?))
                .unwrap()
        };
        assert_eq!(of("aaaaaaaaaaaa", 2, &mark, None), MarkOf::Live);
        assert_eq!(of("aaaaaaaaaaaa", 2, &mark, Some(&mark)), MarkOf::Own);
        // Copied onto another version or artifact, guessed, or garbled.
        assert_eq!(of("aaaaaaaaaaaa", 3, &mark, None), MarkOf::Foreign);
        assert_eq!(of("bbbbbbbbbbbb", 2, &mark, None), MarkOf::Foreign);
        let guessed = step_mark("j1", "publish:1", &"0".repeat(32));
        assert_eq!(of("aaaaaaaaaaaa", 2, &guessed, None), MarkOf::Foreign);
        let renamed = step_mark("j1", "publish:2", n);
        assert_eq!(of("aaaaaaaaaaaa", 2, &renamed, None), MarkOf::Foreign);
        assert_eq!(
            of("aaaaaaaaaaaa", 2, r#"["j1","publish:1"]"#, None),
            MarkOf::Foreign
        );
        // Its key committed, or its job gone: it can never commit.
        st.with_write(|c| {
            Ok(c.execute(
                "INSERT INTO job_steps (job_id, key, result_json, created_at)
                 VALUES ('j1', 'publish:1', '{}', 'x')",
                [],
            )?)
        })
        .unwrap();
        assert_eq!(of("aaaaaaaaaaaa", 2, &mark, Some(&mark)), MarkOf::Dead);
        st.with_write(|c| Ok(c.execute("DELETE FROM jobs WHERE id = 'j1'", [])?))
            .unwrap();
        assert_eq!(of("aaaaaaaaaaaa", 2, &mark, None), MarkOf::Dead);
    }

    /// A store on one connection whose content writes `gated` holds.
    fn gated_store(gated: Arc<crate::test_util::Gated>) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        let conn = rusqlite::Connection::open(home.db_path()).unwrap();
        let st = Store::open_on(
            &home,
            conn,
            Arc::new(clax_core::working::SystemClock),
            gated,
        )
        .unwrap();
        (dir, st)
    }

    #[test]
    fn a_key_committed_by_an_overlapping_call_is_replayed_not_moved() {
        use std::future::Future;
        let gated = Arc::new(crate::test_util::Gated::new());
        let (_d, st) = gated_store(gated.clone());
        let aid = artifact(&st, None);
        change(&st, "j1", &aid);
        let step = || publish_step(Some(&aid), 1, "<p>v2", false);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        // The first call holds the content lock, waiting on its write; the
        // second has passed its replay check and waits for the lock.
        gated.set_open(false);
        let mut first =
            std::pin::pin!(st.commit_publish_step_async(DAEMON, "j1", "publish:1", step()));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        let mut second =
            std::pin::pin!(st.commit_publish_step_async(DAEMON, "j1", "publish:1", step()));
        assert!(second.as_mut().poll(&mut cx).is_pending());
        gated.set_open(true);
        let first = clax_core::content::wait(first).unwrap();
        assert!(!first.replayed);
        let second = clax_core::content::wait(second).unwrap();
        assert!(second.replayed, "the second call answers the first's step");
        assert_eq!(second.step, first.step);
        assert_eq!(second.step.version_n, Some(2));
        assert_eq!(versions(&st, &aid), vec![1, 2]);
    }

    #[test]
    fn a_final_job_refuses_a_new_step_and_replays_an_old_one() {
        let (_d, st) = store();
        running(&st, "j1");
        let first = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        let e = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:1",
                publish_step(None, 0, "<p>b", true),
            )
            .unwrap_err();
        assert!(matches!(e, StepError::Job(JobError::Moved(_))), "{e:?}");
        assert_eq!(st.list_artifacts().unwrap().len(), 1);
        let again = st
            .commit_publish_step(
                DAEMON,
                "j1",
                "publish:0",
                publish_step(None, 0, "<p>a", true),
            )
            .unwrap();
        assert_eq!(again.step, first.step);
    }

    fn reply_step(threads: &[&str]) -> ReplyStep {
        ReplyStep {
            threads: threads.iter().map(|t| t.to_string()).collect(),
            body: "Made it green.".into(),
            resolve: true,
            ack: Vec::new(),
            done: true,
        }
    }

    /// A change `id` of `aid` (at version 1), running, whose publish step
    /// made version 2 from session `sid`.
    fn published_change(st: &Store, id: &str, aid: &ArtifactId, sid: &str) -> JobRow {
        change(st, id, aid);
        let mut step = publish_step(Some(aid), 1, "<p>v2", false);
        step.session_id = Some(sid.into());
        st.commit_publish_step(DAEMON, id, "publish:1", step)
            .unwrap()
            .job
    }

    #[test]
    fn a_reply_step_replies_resolves_and_acks_once() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        st.send_to_agent(DAEMON, &t).unwrap();
        let sid = session(&st, "clax-app", "h1");
        published_change(&st, "j1", &aid, &sid);
        let c = st
            .commit_reply_step(DAEMON, "j1", "reply:0", reply_step(&[&t]))
            .unwrap();
        assert!(!c.replayed);
        let made = c.made.unwrap();
        assert_eq!(made.comments.len(), 1);
        assert_eq!(c.step.result["comments"][0]["thread_id"], json!(t));
        assert_eq!(c.job.state, JobState::Done);
        let thread = st.get_thread(&t).unwrap().unwrap();
        assert_eq!(thread.status, "resolved");
        let agent = |th: &clax_core::model::Thread| {
            th.comments
                .iter()
                .filter(|c| c.author_kind == super::super::threads::AUTHOR_AGENT)
                .count()
        };
        assert_eq!(agent(&thread), 1);
        let again = st
            .commit_reply_step(DAEMON, "j1", "reply:0", reply_step(&[&t]))
            .unwrap();
        assert!(again.replayed);
        assert_eq!(agent(&st.get_thread(&t).unwrap().unwrap()), 1);
    }

    #[test]
    fn a_reply_step_after_a_relaunch_comes_from_the_session_that_published() {
        let (_d, st) = store();
        let published = session(&st, "clax-app", "h1");
        let aid = artifact(&st, Some(&published));
        let t = thread_on(&st, &aid);
        st.send_to_agent(DAEMON, &t).unwrap();
        published_change(&st, "j1", &aid, &published);
        // The app is killed and relaunched: its worker registers a new
        // session, and the reply step names none.
        let relaunched = session(&st, "clax-app", "h2");
        assert_eq!(st.feedback_rows(&t).unwrap().len(), 1);
        let step = ReplyStep {
            resolve: false,
            ..reply_step(&[&t])
        };
        let c = st.commit_reply_step(DAEMON, "j1", "reply", step).unwrap();
        let made = c.made.unwrap();
        assert_eq!(made.session_id, published);
        let rows = st.feedback_rows(&t).unwrap();
        let of = |sid: &str| {
            rows.iter()
                .find(|f| f.target_session_id.as_deref() == Some(sid))
        };
        assert!(
            of(&published).is_some_and(|f| f.acknowledged_at.is_some()),
            "{rows:?}"
        );
        assert!(of(&relaunched).is_none(), "{rows:?}");
    }

    #[test]
    fn a_reply_step_needs_the_jobs_publish_step_and_its_live_session() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        let before = change(&st, "j1", &aid);
        let code = |e: StepError| match e {
            StepError::Store(CoreError::Invalid { code, .. }) => code,
            e => panic!("{e:?}"),
        };
        let e = st
            .commit_reply_step(DAEMON, "j1", "reply", reply_step(&[&t]))
            .unwrap_err();
        assert_eq!(code(e), "invalid_step");
        assert_eq!(st.job("j1").unwrap().unwrap().job, before);
        // A version published from no session: no session to reply from.
        st.commit_publish_step(
            DAEMON,
            "j1",
            "publish:1",
            publish_step(Some(&aid), 1, "<p>v2", false),
        )
        .unwrap();
        let e = st
            .commit_reply_step(DAEMON, "j1", "reply", reply_step(&[&t]))
            .unwrap_err();
        assert_eq!(code(e), "unknown_session");
    }

    #[test]
    fn a_session_an_open_job_names_outlives_the_reaper_until_the_job_ends() {
        let clock = Arc::new(clax_core::working::ManualClock::at("2026-10-19T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        st.send_to_agent(DAEMON, &t).unwrap();
        let sid = session(&st, "clax-app", "h1");
        change(&st, "j1", &aid);
        let mut step = publish_step(Some(&aid), 1, "<p>v2", false);
        step.session_id = Some(sid.clone());
        st.commit_publish_step(DAEMON, "j1", "publish:1", step)
            .unwrap();
        // The app is killed and relaunched more than five minutes later.
        let idle = std::time::Duration::from_secs(Store::SESSION_IDLE_SECS);
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert!(!reaped.ended.contains(&sid), "{:?}", reaped.ended);
        let c = st
            .commit_reply_step(DAEMON, "j1", "reply:1", reply_step(&[&t]))
            .unwrap();
        assert_eq!(c.job.state, JobState::Done);
        // The job ended: the session is reaped as any other.
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert_eq!(reaped.ended, [sid]);
    }

    #[test]
    fn a_begun_publish_steps_session_outlives_the_reaper_until_it_commits() {
        let clock = Arc::new(clax_core::working::ManualClock::at("2026-10-19T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let sid = session(&st, "clax-app", "h1");
        running(&st, "j1");
        let mut step = publish_step(None, 0, "<p>a", true);
        step.session_id = Some(sid.clone());
        // The app is killed in the step's first attempt, before its commit.
        let cut = |_: &Transaction<'_>| Err(CoreError::invalid("cut", "the process died"));
        cut_step(&st, "j1", "publish:0", step, cut).unwrap_err();
        // Relaunched more than five minutes later.
        let idle = std::time::Duration::from_secs(Store::SESSION_IDLE_SECS);
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert!(!reaped.ended.contains(&sid), "{:?}", reaped.ended);
        let mut step = publish_step(None, 0, "<p>a", true);
        step.session_id = Some(sid.clone());
        let c = st
            .commit_publish_step(DAEMON, "j1", "publish:0", step)
            .unwrap();
        assert_eq!(
            c.made.unwrap().version.session_id.as_deref(),
            Some(sid.as_str())
        );
        // Committed and done: the session is reaped as any other.
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert_eq!(reaped.ended, [sid]);
    }

    #[test]
    fn a_waiting_jobs_question_outlives_the_reaper_until_it_closes() {
        let clock = Arc::new(clax_core::working::ManualClock::at("2026-10-19T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let q = question(&st);
        let sid = asker(&st, &q);
        running(&st, "j1");
        assert_eq!(
            st.ask_job_question("j1", &q).unwrap().state,
            JobState::Waiting
        );
        // The app is killed and relaunched more than five minutes later,
        // before the person answers.
        let idle = std::time::Duration::from_secs(Store::SESSION_IDLE_SECS);
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert!(!reaped.ended.contains(&sid), "{:?}", reaped.ended);
        assert!(reaped.withdrawn_questions.is_empty());
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(full.job.state, JobState::Waiting);
        assert_eq!(full.questions[0].answer, JobAnswer::Pending);
        assert_eq!(open_questions(&st), 1, "the person is still asked");
        // The answer moves the job back to running with it.
        answer(&st, &q);
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!(full.job.state, JobState::Running);
        assert!(matches!(full.questions[0].answer, JobAnswer::Answered(_)));
        // The question closed: its asker is reaped as any other.
        clock.advance(Store::SESSION_IDLE_SECS as i64 + 60);
        let reaped = st.reap_sessions(idle, &|_| false).unwrap();
        assert_eq!(reaped.ended, [sid]);
    }

    #[test]
    fn a_reply_step_on_another_artifacts_thread_writes_nothing() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let other = artifact(&st, None);
        let mine = thread_on(&st, &aid);
        let theirs = thread_on(&st, &other);
        let sid = session(&st, "clax-app", "h1");
        let before = published_change(&st, "j1", &aid, &sid);
        let e = st
            .commit_reply_step(DAEMON, "j1", "reply:0", reply_step(&[&mine, &theirs]))
            .unwrap_err();
        assert!(
            matches!(
                e,
                StepError::Store(CoreError::Invalid {
                    code: "invalid_thread",
                    ..
                })
            ),
            "{e:?}"
        );
        assert_eq!(st.get_thread(&mine).unwrap().unwrap().comments.len(), 1);
        assert_eq!(st.get_thread(&mine).unwrap().unwrap().status, "open");
        assert_eq!(st.job("j1").unwrap().unwrap().job, before);
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
                tool_use_id: None,
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
    fn a_committed_step_clears_the_error_of_the_step_before_it() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        // A job failed by a refused step, then retried: the error stays on
        // the row, naming what refused the step, until the next one commits.
        let refused = json!({"code": "doc_moved", "detail": "notes.json", "step": "publish:1"});
        let fail = |id: &str| {
            let rev = st.job(id).unwrap().unwrap().job.rev;
            st.patch_job(
                id,
                JobPatch {
                    state: Some(JobState::Failed),
                    error: Some(Some(refused.clone())),
                    ..patch(rev)
                },
            )
            .unwrap();
            let retried = move_to(&st, id, JobState::Running).unwrap();
            assert_eq!(retried.error, Some(refused.clone()), "kept by the retry");
        };
        running(&st, "j1");
        fail("j1");
        let (_, row, _) = st.commit_step("j1", "turn:2", json!({}), false).unwrap();
        assert_eq!(row.error, None);
        change(&st, "j2", &aid);
        fail("j2");
        let c = st
            .commit_publish_step(
                DAEMON,
                "j2",
                "publish:1",
                publish_step(Some(&aid), 1, "<p>v2", false),
            )
            .unwrap();
        assert_eq!(c.job.error, None);
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
        let q = question_for(&st, "h1", Source::Ask, Some("turn:2/a1".into()));
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
        assert_eq!(
            j1.questions,
            [JobQuestion {
                question_id: q.clone(),
                tool_use_id: Some("turn:2/a1".into()),
                answer: JobAnswer::Pending,
            }],
            "each question with the tool call that asked it"
        );
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
        // The reaper spares the asker of a question the job waits on.
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
        assert!(reaped.ended.is_empty(), "{:?}", reaped.ended);
        assert_eq!(st.job("j1").unwrap().unwrap().job.state, JobState::Waiting);
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
    fn deleting_a_thread_leaves_done_and_cancelled_jobs_as_they_were() {
        let (_d, st) = store();
        let aid = artifact(&st, None);
        let t = thread_on(&st, &aid);
        running(&st, "j1");
        st.set_job_thread("j1", &t, "c1", false).unwrap();
        let (_, done, _) = st.commit_step("j1", "reply:0", json!({}), true).unwrap();
        running(&st, "j2");
        st.set_job_thread("j2", &t, "c1", false).unwrap();
        let cancelled = move_to(&st, "j2", JobState::Cancelled).unwrap();
        running(&st, "j3");
        let answering = st.set_job_thread("j3", &t, "c1", false).unwrap();
        let heard = heard(&st);
        st.delete_thread(DAEMON, &t).unwrap();
        assert_eq!(st.job("j1").unwrap().unwrap().job, done);
        assert_eq!(st.job("j2").unwrap().unwrap().job, cancelled);
        let open = st.job("j3").unwrap().unwrap();
        assert_eq!(open.job.rev, answering.rev + 1);
        for j in ["j1", "j2", "j3"] {
            assert!(st.job(j).unwrap().unwrap().threads.is_empty(), "{j}");
        }
        assert_eq!(
            heard
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["j3"]
        );
    }

    #[test]
    fn a_soft_deleted_artifact_takes_its_consents_and_renders() {
        let (_d, st) = store();
        let (a, b, c) = (
            artifact(&st, None),
            artifact(&st, None),
            artifact(&st, None),
        );
        let (a, b, c) = (a, b.as_str().to_string(), c.as_str().to_string());
        st.put_agent_read_consent(a.as_str(), &b).unwrap();
        st.put_agent_read_consent(&b, a.as_str()).unwrap();
        st.put_agent_read_consent(&b, &c).unwrap();
        st.with_write(|db| {
            for d in [a.as_str(), b.as_str()] {
                db.execute(
                    "INSERT INTO draft_renders (draft_id, digest, clean, created_at)
                     VALUES (?1, 'ab', 1, 'x')",
                    [d],
                )?;
            }
            Ok(())
        })
        .unwrap();
        st.delete_artifact(DAEMON, &a).unwrap();
        assert_eq!(st.agent_read_consents().unwrap(), vec![(b.clone(), c)]);
        let renders: Vec<String> = st
            .with_read(|db| {
                Ok(db
                    .prepare("SELECT draft_id FROM draft_renders")?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(renders, vec![b]);
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
