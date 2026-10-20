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
use clax_core::CoreError;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

/// The states a job may be in, as the `jobs.state` check lists them.
pub const STATES: [&str; 8] = [
    "queued",
    "running",
    "waiting",
    "paused",
    "stopped",
    "failed",
    "cancelled",
    "done",
];

/// A job's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JobRow {
    pub id: String,
    /// Who runs it (`app` for the app's queue).
    pub client: String,
    /// `make` or `change`; opaque to the core.
    pub kind: String,
    pub artifact_id: Option<String>,
    /// One of [`STATES`].
    pub state: String,
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

/// A question a job asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobQuestion {
    pub question_id: String,
    /// `None` while unanswered; else JSON text: the answers, as the
    /// question keeps them, or `null` for a decline.
    pub answer: Option<String>,
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
    pub state: Option<String>,
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
    /// The job is no longer at the `rev` the write named; its row now.
    Moved(Box<JobRow>),
    /// A job of that ID exists with another body.
    Conflict,
    /// No job of that ID.
    NotFound,
    /// The store failed, or the write named something invalid
    /// ([`CoreError::Invalid`]: a state, an artifact or a thread).
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
    Ok(JobRow {
        id: r.get(0)?,
        client: r.get(1)?,
        kind: r.get(2)?,
        artifact_id: r.get(3)?,
        state: r.get(4)?,
        step: r.get(5)?,
        title: r.get(6)?,
        input: json(r.get(7)?)?,
        plan: r.get::<_, Option<String>>(8)?.map(json).transpose()?,
        carry: r.get::<_, Option<String>>(9)?.map(json).transpose()?,
        error: r.get::<_, Option<String>>(10)?.map(json).transpose()?,
        rev: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

/// Job `id`'s row, or [`JobError::NotFound`].
fn existing(c: &Connection, id: &str) -> JobResult<JobRow> {
    job_row(c, id)?.ok_or(JobError::NotFound)
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
            "SELECT question_id, answer FROM job_questions
             WHERE job_id = ?1 ORDER BY question_id",
        )?
        .query_map(params![job.id], |r| {
            Ok(JobQuestion {
                question_id: r.get(0)?,
                answer: r.get(1)?,
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

    /// Applies `p` to job `id` when it is at `p.if_rev`.
    ///
    /// # Errors
    /// [`JobError::Moved`] with the current row when it is not;
    /// [`JobError::NotFound`]; [`CoreError::Invalid`] `invalid_state` for a
    /// state not in [`STATES`].
    pub fn patch_job(&self, id: &str, p: JobPatch) -> JobResult<JobRow> {
        if let Some(state) = &p.state
            && !STATES.contains(&state.as_str())
        {
            return Err(
                CoreError::invalid("invalid_state", format!("no job state {state}")).into(),
            );
        }
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
            if row.rev != p.if_rev {
                return Err(JobError::Moved(Box::new(row)));
            }
            let state = p.state.unwrap_or(row.state);
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
            bump(tx, id, &self.now())
        })
    }

    /// Records that job `id` answers thread `thread` up to comment
    /// `comment`, or stopped at it (`stopped`), replacing what it recorded
    /// for that thread before.
    ///
    /// # Errors
    /// [`JobError::NotFound`]; [`CoreError::Invalid`] `invalid_thread` when
    /// `thread` names no thread.
    pub fn set_job_thread(
        &self,
        id: &str,
        thread: &str,
        comment: &str,
        stopped: bool,
    ) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            existing(tx, id)?;
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
    /// [`JobError::NotFound`].
    pub fn remove_job_thread(&self, id: &str, thread: &str) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
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

    /// Records a question the job asked and moves it to `waiting`, in one
    /// transaction. Recording a question the job already asked changes
    /// nothing.
    ///
    /// # Errors
    /// [`JobError::NotFound`].
    pub fn ask_job_question(&self, id: &str, question: &str) -> JobResult<JobRow> {
        self.job_tx(|tx| {
            let row = existing(tx, id)?;
            let n = tx.execute(
                "INSERT OR IGNORE INTO job_questions (job_id, question_id) VALUES (?1, ?2)",
                params![id, question],
            )?;
            if n == 0 {
                return Ok(row);
            }
            tx.execute(
                "UPDATE jobs SET state = 'waiting' WHERE id = ?1",
                params![id],
            )?;
            bump(tx, id, &self.now())
        })
    }

    /// Commits step `key` of job `id` with `result`, setting the job's
    /// `step` to it, and its state to `done` when `done`, in one
    /// transaction. A key the job already has returns its step as recorded
    /// with `replayed = true`, and changes nothing.
    ///
    /// # Errors
    /// [`JobError::NotFound`].
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
            let now = self.now();
            tx.execute(
                "INSERT INTO job_steps (job_id, key, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, key, text(&result), now],
            )?;
            tx.execute(
                "UPDATE jobs SET step = ?2, state = CASE WHEN ?3 THEN 'done' ELSE state END
                 WHERE id = ?1",
                params![id, key, done],
            )?;
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

    /// Stores `answer` (the answers' JSON text, or `None` for a decline) of
    /// question `question` in the job that asked it, and moves that job
    /// from `waiting` back to `running` once it waits on no other question,
    /// inside the caller's transaction; the job's row after it, or `None`
    /// when no job asked it or its answer is already stored. Called by
    /// [`Store::close_question`] for an answer or a decline.
    pub(crate) fn answer_job_question_tx(
        &self,
        tx: &Transaction<'_>,
        question: &str,
        answer: Option<&str>,
    ) -> clax_core::Result<Option<JobRow>> {
        let id: Option<String> = tx
            .query_row(
                "SELECT job_id FROM job_questions
                 WHERE question_id = ?1 AND answer IS NULL ORDER BY job_id LIMIT 1",
                params![question],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE job_questions SET answer = ?3 WHERE job_id = ?1 AND question_id = ?2",
            params![id, question, answer.unwrap_or("null")],
        )?;
        tx.execute(
            "UPDATE jobs SET state = 'running'
             WHERE id = ?1 AND state = 'waiting' AND NOT EXISTS
                (SELECT 1 FROM job_questions WHERE job_id = ?1 AND answer IS NULL)",
            params![id],
        )?;
        match bump(tx, &id, &self.now()) {
            Ok(row) => Ok(Some(row)),
            Err(JobError::Store(e)) => Err(e),
            Err(_) => Ok(None),
        }
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
        let sid = session(st, "app", "h1");
        let qs = serde_json::from_value(json!([{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]))
        .unwrap();
        st.create_question(
            DAEMON,
            NewQuestion {
                session_id: sid,
                artifact_id: None,
                source: Source::Ask,
                tool_use_id: None,
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
            (a.state.as_str(), a.rev, a.step.as_deref()),
            ("queued", 1, None)
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
                    state: Some("running".into()),
                    ..patch(1)
                },
            )
            .unwrap();
        assert_eq!((running.state.as_str(), running.rev), ("running", 2));
        let stale = st.patch_job(
            "j1",
            JobPatch {
                state: Some("paused".into()),
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
        assert!(matches!(
            st.patch_job(
                "j1",
                JobPatch {
                    state: Some("sleeping".into()),
                    ..patch(2)
                }
            ),
            Err(JobError::Store(CoreError::Invalid {
                code: "invalid_state",
                ..
            }))
        ));
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
                    state: Some("running".into()),
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
                state: Some("running".into()),
                ..patch(1)
            },
        )
        .unwrap();
        let asked = st.ask_job_question("j1", &q).unwrap();
        assert_eq!((asked.state.as_str(), asked.rev), ("waiting", 3));
        // Asking it again changes nothing.
        assert_eq!(st.ask_job_question("j1", &q).unwrap(), asked);
        assert_eq!(
            st.job("j1").unwrap().unwrap().questions,
            vec![JobQuestion {
                question_id: q.clone(),
                answer: None
            }]
        );
        answer(&st, &q);
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!((full.job.state.as_str(), full.job.rev), ("running", 4));
        let stored = full.questions[0].answer.as_deref().unwrap();
        let stored: serde_json::Value = serde_json::from_str(stored).unwrap();
        assert_eq!(stored[0]["selected"], json!(["A"]));
    }

    #[test]
    fn a_declined_question_moves_the_job_back_too() {
        let (_d, st) = store();
        let q = question(&st);
        st.create_job(new_job("j1")).unwrap();
        st.ask_job_question("j1", &q).unwrap();
        st.close_question(DAEMON, &q, Close::Decline).unwrap();
        let full = st.job("j1").unwrap().unwrap();
        assert_eq!((full.job.state.as_str(), full.job.rev), ("running", 3));
        assert_eq!(full.questions[0].answer.as_deref(), Some("null"));
    }

    #[test]
    fn an_answer_moves_only_a_waiting_job_and_only_once_it_waits_on_nothing() {
        let (_d, st) = store();
        let (q1, q2) = (question(&st), question(&st));
        st.create_job(new_job("j1")).unwrap();
        st.ask_job_question("j1", &q1).unwrap();
        st.ask_job_question("j1", &q2).unwrap();
        answer(&st, &q1);
        let j = st.job("j1").unwrap().unwrap().job;
        assert_eq!(
            (j.state.as_str(), j.rev),
            ("waiting", 4),
            "still waits on q2"
        );
        let paused = st
            .patch_job(
                "j1",
                JobPatch {
                    state: Some("paused".into()),
                    ..patch(4)
                },
            )
            .unwrap();
        answer(&st, &q2);
        let j = st.job("j1").unwrap().unwrap().job;
        assert_eq!((j.state.as_str(), j.rev), ("paused", paused.rev + 1));
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
            (row.state.as_str(), row.step.as_deref()),
            ("done", Some("reply:0"))
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
                state: Some("cancelled".into()),
                ..patch(1)
            },
        )
        .unwrap();
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
        assert_eq!(j1.job.state, "waiting");
        assert!(open[1].steps.is_empty() && open[1].threads.is_empty());
        assert_eq!(st.open_jobs("cli").unwrap().len(), 1);
        assert!(st.job("nope").unwrap().is_none());
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
        assert!(st.agent_read_consents().unwrap().is_empty());
        st.put_agent_read_consent("8r4m2nzy3c5v", "7q3k9mzx2b4t")
            .unwrap();
        st.put_agent_read_consent("8r4m2nzy3c5v", "7q3k9mzx2b4t")
            .unwrap();
        st.put_agent_read_consent("7q3k9mzx2b4t", "8r4m2nzy3c5v")
            .unwrap();
        assert_eq!(
            st.agent_read_consents().unwrap(),
            vec![
                ("7q3k9mzx2b4t".to_string(), "8r4m2nzy3c5v".to_string()),
                ("8r4m2nzy3c5v".to_string(), "7q3k9mzx2b4t".to_string()),
            ]
        );
        assert!(
            st.delete_agent_read_consent("8r4m2nzy3c5v", "7q3k9mzx2b4t")
                .unwrap()
        );
        assert!(
            !st.delete_agent_read_consent("8r4m2nzy3c5v", "7q3k9mzx2b4t")
                .unwrap()
        );
        assert_eq!(st.agent_read_consents().unwrap().len(), 1);
    }
}
