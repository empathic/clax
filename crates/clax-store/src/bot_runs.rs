//! ClaxBot runs and their steps (migration 50, spec
//! 2026-10-21-claxbot-design §4.2, §6.3).
//!
//! A **run** is ClaxBot working one request for one person. It moves
//! `queued` → `running` (a driver claimed it) → `finished`, `gave_up` or
//! `stopped`; a cancel makes it `stopping` until the driver writes its
//! `stopped` step. A run is a sequence of **steps**, each begun
//! ([`Store::begin_step`]) and then committed ([`Store::commit_bot_step`])
//! before the next begins, so a run survives a restart: a step begun but
//! not committed is found again ([`Store::open_step`]) and executed again.
//!
//! Steps are **fenced**: a driver begins and commits a step only while it
//! holds the run's claim ([`Store::claim_run`], kept alive across a long
//! turn by [`Store::renew_lease`]), and names the step it expects, so a
//! driver whose claim another took over, or that lost track of the run
//! after a crash, is refused (`run_not_claimed`, `step_mismatch`) instead
//! of running or committing a step twice. A driver is a [`DriverId`],
//! unique per process natively (minted when the daemon starts) and per
//! Durable Object incarnation on the Worker (minted when the instance
//! object is constructed); a driver ID shared by two executors would let
//! both hold one claim, so it is never reused.
//!
//! A step's commit is one transaction: the step's payload, its usage rows
//! ([`crate::credits`]), its draft changes, the run's running credit total
//! and status, and its audit events. The store stamps each event with the
//! run's artifact and its `run` and `step`, fills a `bot.turn`'s or
//! `bot.script`'s `credits_milli` with the step's charge, and records
//! `bot.run_start` when a run is made and `bot.run_end` when a terminal
//! step (`finish`, `give_up`, `stopped`) commits.
//!
//! A run's **draft** is an overlay on the version it was based on
//! (`base_version`): each path it wrote, with its content, or deleted.
//! The artifact changes only when the run publishes it, through the
//! artifact's own publish path.

use super::Store;
use super::credits::{KeySource, Metered, UsageRow, charge_in, check_model_priced};
use super::roles::set_by;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::Value;

/// Declares a string enum with its stored names.
macro_rules! named {
    ($(#[$m:meta])* $name:ident { $($v:ident = $s:literal,)* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($v,)*
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v,)*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$v => $s,)*
                }
            }

            pub fn parse(s: &str) -> Option<$name> {
                match s {
                    $($s => Some($name::$v),)*
                    _ => None,
                }
            }
        }
    };
}

named! {
    /// Where a run is.
    RunStatus {
        Queued = "queued",
        Running = "running",
        Stopping = "stopping",
        Finished = "finished",
        GaveUp = "gave_up",
        Stopped = "stopped",
        Failed = "failed",
    }
}

impl RunStatus {
    /// Whether the run has ended.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Finished | RunStatus::GaveUp | RunStatus::Stopped | RunStatus::Failed
        )
    }
}

named! {
    /// What started a run.
    RequestKind {
        Thread = "thread",
        Brief = "brief",
    }
}

named! {
    /// What a step is. `failed` ends a run that could not go on (its step
    /// failed to commit too often).
    StepKind {
        Turn = "turn",
        Script = "script",
        Finish = "finish",
        GiveUp = "give_up",
        Stopped = "stopped",
        Failed = "failed",
    }
}

impl StepKind {
    /// The status a committed step of this kind leaves its run in, for a
    /// terminal step.
    pub fn ends_as(self) -> Option<RunStatus> {
        match self {
            StepKind::Finish => Some(RunStatus::Finished),
            StepKind::GiveUp => Some(RunStatus::GaveUp),
            StepKind::Stopped => Some(RunStatus::Stopped),
            StepKind::Failed => Some(RunStatus::Failed),
            StepKind::Turn | StepKind::Script => None,
        }
    }
}

/// One run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run {
    /// A ULID.
    pub id: String,
    pub artifact_id: String,
    /// The person key the run acts for.
    pub person: String,
    pub request: RequestKind,
    pub thread_ids: Vec<String>,
    pub status: RunStatus,
    /// Why it stopped or gave up (`credits_exhausted`, `run_cap`, …).
    pub reason: Option<String>,
    pub model: String,
    pub key_source: KeySource,
    /// The version its draft is based on.
    pub base_version: u32,
    /// Credits charged so far.
    pub credits_milli: i64,
    /// The driver holding the run, and since when.
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    /// When a driver that let the run wait (a stalled sandbox's backoff,
    /// an unreachable model's retry) is to look at it again; `None`: at
    /// once. A driver that starts honours it ([`Store::set_run_due`]).
    pub due_at: Option<String>,
    /// The grant of the agent that asked for the run (a person's agent of
    /// another machine, or an agent token), whose rights bound the run's
    /// as much as its person's do; `None` for a person's own send and the
    /// owner's agents.
    pub agent_grant: Option<String>,
}

/// One step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Step {
    pub run_id: String,
    /// From 1.
    pub n: i64,
    pub kind: StepKind,
    /// When the step was first begun (its later attempts keep it).
    pub started_at: String,
    /// How many times the step was begun: 1, and one more each time it is
    /// begun again after a driver lost it uncommitted. A step's live events
    /// are told apart by it (`<step>.<attempt>:<seq>`).
    pub attempt: i64,
    /// `None` until committed: such a step is executed again on resume.
    pub committed_at: Option<String>,
    /// What the step did (the turn's messages, the script's code and
    /// outcome), once committed.
    pub payload: Option<Value>,
    /// What the model calls of the step's attempts reported they used, as
    /// they streamed ([`Store::observe_step`]): one entry per attempt.
    pub observed: Vec<Value>,
    /// How many times committing the step failed ([`Store::note_commit_failure`]).
    pub failures: i64,
}

/// A run to make.
#[derive(Clone, Debug)]
pub struct NewRun {
    pub artifact_id: String,
    pub person: String,
    pub request: RequestKind,
    pub thread_ids: Vec<String>,
    pub model: String,
    pub key_source: KeySource,
    pub base_version: u32,
    /// The grant of the agent that asked for it ([`Run::agent_grant`]).
    pub agent_grant: Option<String>,
}

/// A change to a run's draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftChange {
    Write {
        path: String,
        content: Vec<u8>,
        content_type: Option<String>,
    },
    Delete {
        path: String,
    },
}

/// One path of a draft's overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftFile {
    pub path: String,
    /// `None`: the draft deleted the path.
    pub content: Option<Vec<u8>>,
    pub content_type: Option<String>,
}

/// What a step's commit writes.
#[derive(Clone, Debug, Default)]
pub struct StepCommit {
    pub payload: Value,
    /// Metered usage, charged to the run's person.
    pub usage: Vec<Metered>,
    /// The step's audit events (`bot.turn`, `bot.script`, `bot.call`, …):
    /// the store sets their artifact, `run` and `step`.
    pub events: Vec<AuditRecord>,
    pub draft: Vec<DraftChange>,
    /// Why a terminal step ended the run.
    pub reason: Option<String>,
}

/// What a step's commit wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Committed {
    pub step: Step,
    pub usage: Vec<UsageRow>,
    /// The step's charge.
    pub credits_milli: i64,
    /// The run after.
    pub run: Run,
}

const RUN_COLS: &str = "id, artifact_id, person, request, thread_ids_json, status, reason, model,
    key_source, base_version, credits_milli, claimed_by, claimed_at, created_at, updated_at, ended_at,
    agent_grant, due_at";

fn row_to_run(r: &Row<'_>) -> rusqlite::Result<Run> {
    let s = |i: usize| r.get::<_, String>(i);
    Ok(Run {
        id: s(0)?,
        artifact_id: s(1)?,
        person: s(2)?,
        request: RequestKind::parse(&s(3)?).unwrap_or(RequestKind::Thread),
        thread_ids: serde_json::from_str(&s(4)?).unwrap_or_default(),
        status: RunStatus::parse(&s(5)?).unwrap_or(RunStatus::Stopped),
        reason: r.get(6)?,
        model: s(7)?,
        key_source: KeySource::parse(&s(8)?).unwrap_or(KeySource::Instance),
        base_version: r.get(9)?,
        credits_milli: r.get(10)?,
        claimed_by: r.get(11)?,
        claimed_at: r.get(12)?,
        created_at: s(13)?,
        updated_at: s(14)?,
        ended_at: r.get(15)?,
        agent_grant: r.get(16)?,
        due_at: r.get(17)?,
    })
}

fn row_to_step(r: &Row<'_>) -> rusqlite::Result<Step> {
    let kind: String = r.get(2)?;
    let payload: Option<String> = r.get(5)?;
    Ok(Step {
        run_id: r.get(0)?,
        n: r.get(1)?,
        kind: StepKind::parse(&kind).unwrap_or(StepKind::Stopped),
        started_at: r.get(3)?,
        attempt: r.get(6)?,
        committed_at: r.get(4)?,
        payload: payload.and_then(|p| serde_json::from_str(&p).ok()),
        observed: r
            .get::<_, Option<String>>(7)?
            .and_then(|o| serde_json::from_str(&o).ok())
            .unwrap_or_default(),
        failures: r.get(8)?,
    })
}

const STEP_COLS: &str =
    "run_id, n, kind, started_at, committed_at, payload_json, attempt, observed_json, failures";

fn run_in(c: &Connection, id: &str) -> Result<Option<Run>> {
    Ok(
        c.prepare_cached(&format!("SELECT {RUN_COLS} FROM bot_runs WHERE id = ?1"))?
            .query_row(params![id], row_to_run)
            .optional()?,
    )
}

/// [`Store::active_run_on`] on `c`.
fn active_on_in(c: &Connection, threads: &[String]) -> Result<Option<Run>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {RUN_COLS} FROM bot_runs
          WHERE status IN ('queued', 'running', 'stopping')
            AND EXISTS (SELECT 1 FROM json_each(thread_ids_json) j
                         WHERE j.value IN (SELECT value FROM json_each(?1)))
          ORDER BY created_at DESC, id DESC LIMIT 1"
    ))?;
    let ids = serde_json::to_string(threads).expect("serialisable IDs");
    Ok(q.query_row(params![ids], row_to_run).optional()?)
}

fn open_step_in(c: &Connection, id: &str) -> Result<Option<Step>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 AND committed_at IS NULL"
    ))?
    .query_row(params![id], row_to_step)
    .optional()?)
}

fn runs_where(
    c: &Connection,
    filter: &str,
    arg: &dyn rusqlite::ToSql,
    limit: u32,
) -> Result<Vec<Run>> {
    let mut q = c.prepare_cached(&format!(
        "SELECT {RUN_COLS} FROM bot_runs WHERE {filter} ORDER BY created_at DESC, id DESC LIMIT ?2"
    ))?;
    let rows = q.query_map(params![arg, limit], row_to_run)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `run_invalid` with `message`.
fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid("run_invalid", message)
}

/// Who drives runs: one per process natively (minted when the daemon
/// starts) and one per Durable Object incarnation on the Worker (minted
/// when the instance object is constructed), never shared or reused, so a
/// claim names exactly one live executor and a restarted one is another
/// driver ([`Store::claim_run`] takes a run over from it only once its
/// lease lapses). [`DriverId::mint`] is the only way to make one outside
/// the store's own tests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DriverId(String);

impl DriverId {
    /// A new driver ID, `drv_` and a ULID: never one minted before by this
    /// process (debug builds assert it).
    pub fn mint() -> DriverId {
        let id = DriverId(format!("drv_{}", clax_core::new_ulid()));
        #[cfg(debug_assertions)]
        {
            static MINTED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
                std::sync::Mutex::new(None);
            let fresh = MINTED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(Default::default)
                .insert(id.0.clone());
            debug_assert!(fresh, "driver ID {} minted twice", id.0);
        }
        id
    }

    /// A driver of a fixed name, for the store's tests.
    #[cfg(test)]
    pub(crate) fn named(name: &str) -> DriverId {
        DriverId(name.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DriverId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `run_not_claimed` unless `driver` holds run `run`'s claim.
fn check_claim(run: &Run, driver: &str) -> Result<()> {
    if run.claimed_by.as_deref() == Some(driver) {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "run_not_claimed",
            format!(
                "driver {driver} does not hold run {}; claim it again",
                run.id
            ),
        ))
    }
}

/// The step a driver of run `id` works on next on `c`: its open step, else
/// the one after the last.
fn next_n_in(c: &Connection, id: &str) -> Result<i64> {
    Ok(match open_step_in(c, id)? {
        Some(open) => open.n,
        None => c.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM bot_steps WHERE run_id = ?1",
            params![id],
            |r| r.get(0),
        )?,
    })
}

/// `step_mismatch` naming the step run `id` is at, unless it is `n`.
fn check_step(c: &Connection, id: &str, n: i64) -> Result<()> {
    let at = next_n_in(c, id)?;
    if at == n {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "step_mismatch",
            format!("run {id} is at step {at}, not {n}; read the run again"),
        ))
    }
}

impl Store {
    /// Makes a queued run, recording `bot.run_start {run, request,
    /// thread_ids, model, key_source}` on its artifact under `ctx`.
    ///
    /// # Errors
    /// `NotFound` for an artifact that does not exist; `model_not_priced`
    /// for a model without its token cost lines (an admin prices it with
    /// [`Store::price_model`]); the store's.
    pub fn create_run(&self, ctx: &AuditCtx, new: NewRun) -> Result<Run> {
        self.with_tx(|tx| self.create_run_in(tx, ctx, new))
    }

    /// [`Store::create_run`] unless a run that has not ended (queued,
    /// running or stopping) names one of `new`'s threads: then that run,
    /// and nothing is made, in the same transaction, so two sends of one
    /// thread never make two runs.
    ///
    /// # Errors
    /// [`Store::create_run`]'s.
    pub fn create_run_exclusive(
        &self,
        ctx: &AuditCtx,
        new: NewRun,
    ) -> Result<std::result::Result<Run, Run>> {
        self.with_tx(|tx| {
            if let Some(active) = active_on_in(tx, &new.thread_ids)? {
                return Ok(Err(active));
            }
            Ok(Ok(self.create_run_in(tx, ctx, new)?))
        })
    }

    /// Queues `person` (a person key) for a follow-up run of thread `tid`
    /// once the run working on it ends, behind those queued before (a
    /// person already queued for it keeps their place), in the
    /// transaction that finds that run has not ended: so the run's end,
    /// committed after, always finds them. Returns that run, or `None`,
    /// queueing nothing, when no run works on `tid`.
    ///
    /// # Errors
    /// The store's.
    pub fn queue_follow_up_if_active(&self, tid: &str, person: &str) -> Result<Option<Run>> {
        self.with_tx(|tx| {
            let Some(run) = active_on_in(tx, std::slice::from_ref(&tid.to_string()))? else {
                return Ok(None);
            };
            tx.execute(
                "INSERT OR IGNORE INTO bot_follow_ups (thread_id, person, queued_at)
                 VALUES (?1, ?2, ?3)",
                params![tid, person, self.now()],
            )?;
            Ok(Some(run))
        })
    }

    /// Drops every follow-up queued on `tid` (a thread that can no longer
    /// be sent: deleted, resolved, or of a deleted artifact).
    ///
    /// # Errors
    /// The store's.
    pub fn drop_follow_ups(&self, tid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM bot_follow_ups WHERE thread_id = ?1",
                params![tid],
            )?;
            Ok(())
        })
    }

    /// The threads with a follow-up queued and no run working on them (a
    /// run ended without starting its follow-ups, as a crash after its
    /// last step leaves it), oldest queued first.
    ///
    /// # Errors
    /// The store's.
    pub fn waiting_follow_up_threads(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT thread_id FROM bot_follow_ups GROUP BY thread_id ORDER BY MIN(seq)",
            )?;
            let tids = q
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            let mut out = Vec::new();
            for tid in tids {
                if active_on_in(c, std::slice::from_ref(&tid))?.is_none() {
                    out.push(tid);
                }
            }
            Ok(out)
        })
    }

    /// The first person queued for a follow-up of `tid`
    /// ([`Store::queue_follow_up_if_active`]) as (its place, the person),
    /// left on the queue until [`Store::remove_follow_up`]: a run's end
    /// that finds the thread taken leaves them for that run's end.
    ///
    /// # Errors
    /// The store's.
    pub fn next_follow_up(&self, tid: &str) -> Result<Option<(i64, String)>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT seq, person FROM bot_follow_ups WHERE thread_id = ?1
                 ORDER BY seq LIMIT 1",
            )?
            .query_row(params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
        })
    }

    /// Takes the follow-up at place `seq` ([`Store::next_follow_up`]) off
    /// its queue.
    ///
    /// # Errors
    /// The store's.
    pub fn remove_follow_up(&self, seq: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM bot_follow_ups WHERE seq = ?1", params![seq])?;
            Ok(())
        })
    }

    /// The people queued for a follow-up of `tid`, first first.
    ///
    /// # Errors
    /// The store's.
    pub fn follow_ups(&self, tid: &str) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT person FROM bot_follow_ups WHERE thread_id = ?1 ORDER BY seq",
            )?;
            let v = q
                .query_map(params![tid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(v)
        })
    }

    /// The newest run that has not ended naming one of `threads`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn active_run_on(&self, threads: &[String]) -> Result<Option<Run>> {
        self.with_read(|c| active_on_in(c, threads))
    }

    fn create_run_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        new: NewRun,
    ) -> Result<Run> {
        {
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL",
                    params![new.artifact_id],
                    |r| r.get(0),
                )
                .optional()?;
            exists.ok_or(CoreError::NotFound)?;
            check_model_priced(tx, &new.model)?;
            let id = clax_core::new_ulid();
            let now = self.now();
            tx.execute(
                "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json, status,
                    model, key_source, base_version, created_at, updated_at, agent_grant)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8, ?9, ?9, ?10)",
                params![
                    id,
                    new.artifact_id,
                    new.person,
                    new.request.as_str(),
                    serde_json::to_string(&new.thread_ids).expect("serialisable IDs"),
                    new.model,
                    new.key_source.as_str(),
                    new.base_version,
                    now,
                    new.agent_grant
                ],
            )?;
            let mut rec = AuditRecord::new(AuditKind::BotRunStart, now)
                .with("run", id.as_str())
                .with("request", new.request.as_str())
                .with("thread_ids", new.thread_ids.clone())
                .with("model", new.model.as_str())
                .with("key_source", new.key_source.as_str());
            rec.ids.artifact = Some(new.artifact_id.clone());
            self.record_audit(tx, ctx, rec)?;
            Ok(run_in(tx, &id)?.expect("the run was just made"))
        }
    }

    /// Run `id`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        self.with_read(|c| run_in(c, id))
    }

    /// Run `id` with its steps in order, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn run_with_steps(&self, id: &str) -> Result<Option<(Run, Vec<Step>)>> {
        self.with_read(|c| {
            let Some(run) = run_in(c, id)? else {
                return Ok(None);
            };
            let mut q = c.prepare_cached(&format!(
                "SELECT {STEP_COLS} FROM bot_steps WHERE run_id = ?1 ORDER BY n"
            ))?;
            let steps = q
                .query_map(params![id], row_to_step)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some((run, steps)))
        })
    }

    /// Up to `limit` runs on artifact `aid`, newest first.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_artifact(&self, aid: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| runs_where(c, "artifact_id = ?1", &aid, limit))
    }

    /// Up to `limit` runs acting for `person`, newest first.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_person(&self, person: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| runs_where(c, "person = ?1", &person, limit))
    }

    /// Up to `limit` runs whose request names thread `tid`, newest first:
    /// a thread with one is ClaxBot's.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_for_thread(&self, tid: &str, limit: u32) -> Result<Vec<Run>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {RUN_COLS} FROM bot_runs
                  WHERE EXISTS (SELECT 1 FROM json_each(thread_ids_json) WHERE value = ?1)
                  ORDER BY created_at DESC, id DESC LIMIT ?2"
            ))?;
            let rows = q.query_map(params![tid, limit], row_to_run)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The runs made in the week `at` falls in (Monday 00:00 UTC to the
    /// next), counted by the person they act for.
    ///
    /// # Errors
    /// The store's.
    pub fn runs_in_week(
        &self,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let start = crate::credits::week_start(at);
        let (from, to) = (
            clax_core::time::stamp(start),
            clax_core::time::stamp(crate::credits::week_end(start)),
        );
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT person, count(*) FROM bot_runs WHERE created_at >= ?1 AND created_at < ?2
                  GROUP BY person",
            )?;
            let rows = q.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The credits of up to `limit` of the newest finished runs, acting
    /// for `person` when given, else for anyone: what a run's estimate is
    /// taken from.
    ///
    /// # Errors
    /// The store's.
    pub fn finished_run_credits(&self, person: Option<&str>, limit: u32) -> Result<Vec<i64>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT credits_milli FROM bot_runs
                  WHERE status = 'finished' AND (?1 IS NULL OR person = ?1)
                  ORDER BY ended_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = q.query_map(params![person, limit], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Every run that has not ended (`queued`, `running`, `stopping`),
    /// oldest first: what a driver resumes on start.
    ///
    /// # Errors
    /// The store's.
    pub fn live_runs(&self) -> Result<Vec<Run>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {RUN_COLS} FROM bot_runs WHERE status IN ('queued', 'running', 'stopping')
                  ORDER BY created_at, id"
            ))?;
            let rows = q.query_map([], row_to_run)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Claims run `id` for `driver`: a queued run becomes running. The
    /// claim is taken when the run is unclaimed, already `driver`'s, or
    /// claimed more than `lease_secs` ago (its driver gone; a lease of 0
    /// or less takes it whatever its age); it is renewed each time. Returns the run when `driver` now holds it, `None` when
    /// another driver does or the run has ended.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn claim_run(&self, id: &str, driver: &DriverId, lease_secs: i64) -> Result<Option<Run>> {
        let driver = driver.as_str();
        let now = self.now_utc();
        let stale = clax_core::time::stamp(now - chrono::Duration::seconds(lease_secs.max(0)));
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if run.status.is_terminal() {
                return Ok(None);
            }
            let free = match (&run.claimed_by, &run.claimed_at) {
                (None, _) => true,
                (Some(d), _) if d == driver => true,
                (Some(_), Some(at)) => lease_secs <= 0 || *at < stale,
                (Some(_), None) => true,
            };
            if !free {
                return Ok(None);
            }
            let stamp = clax_core::time::stamp(now);
            tx.execute(
                "UPDATE bot_runs SET claimed_by = ?2, claimed_at = ?3, updated_at = ?3,
                   status = CASE status WHEN 'queued' THEN 'running' ELSE status END
                 WHERE id = ?1",
                params![id, driver, stamp],
            )?;
            run_in(tx, id)
        })
    }

    /// Lets go of `driver`'s claim on run `id`, so another driver may
    /// take it at once.
    ///
    /// # Errors
    /// The store's.
    pub fn release_run(&self, id: &str, driver: &DriverId) -> Result<()> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_runs SET claimed_by = NULL, claimed_at = NULL
                  WHERE id = ?1 AND claimed_by = ?2",
                params![id, driver],
            )?;
            Ok(())
        })
    }

    /// Records when run `id` is next due (`None`: at once), as the driver
    /// that let it wait decided, so a driver that starts later (a restarted
    /// daemon, a new Durable Object incarnation) does not tick it early.
    ///
    /// # Errors
    /// The store's.
    pub fn set_run_due(&self, id: &str, at: Option<chrono::DateTime<chrono::Utc>>) -> Result<()> {
        let at = at.map(clax_core::time::stamp);
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_runs SET due_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            Ok(())
        })
    }

    /// Renews `driver`'s claim on run `id` (now, by the store's clock), so a
    /// turn longer than the lease keeps it. Returns whether `driver` still
    /// holds the run; `false` once another driver took it over or the run
    /// ended, and the driver then stops working on it.
    ///
    /// # Errors
    /// The store's.
    pub fn renew_lease(&self, id: &str, driver: &DriverId) -> Result<bool> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE bot_runs SET claimed_at = ?3 WHERE id = ?1 AND claimed_by = ?2
                   AND status IN ('queued', 'running', 'stopping')",
                params![id, driver, self.now()],
            )? > 0)
        })
    }

    /// The step a driver of run `id` works on next: its open step, else the
    /// one after the last (1 for a new run).
    ///
    /// # Errors
    /// The store's.
    pub fn next_step_n(&self, id: &str) -> Result<i64> {
        self.with_read(|c| next_n_in(c, id))
    }

    /// The step of run `id` begun and not committed: the one to execute
    /// again on resume.
    ///
    /// # Errors
    /// The store's.
    pub fn open_step(&self, id: &str) -> Result<Option<Step>> {
        self.with_read(|c| open_step_in(c, id))
    }

    /// Begins step `n` of run `id` as `kind`, for `driver`, which holds
    /// the run's claim and expects the run at step `n`
    /// ([`Store::next_step_n`]). When step `n` is already begun and not
    /// committed, that step is the one returned, with `kind` and its
    /// `attempt` one more, its `started_at` the first attempt's (nothing of
    /// it was committed, so it is executed again).
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` when `driver` does not hold the run;
    /// `step_mismatch` when the run is at another step; `run_invalid` for a
    /// run that is not running or stopping, or a `stopping` run's step that
    /// is not `stopped`; `model_not_priced` for a turn whose model lost its
    /// cost lines; the store's.
    pub fn begin_step(&self, id: &str, driver: &DriverId, n: i64, kind: StepKind) -> Result<Step> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver)?;
            check_step(tx, id, n)?;
            if kind == StepKind::Turn {
                check_model_priced(tx, &run.model)?;
            }
            match run.status {
                RunStatus::Running => {}
                RunStatus::Stopping if matches!(kind, StepKind::Stopped | StepKind::Failed) => {}
                RunStatus::Stopping => {
                    return Err(invalid("a stopping run's next step is its stopped step"));
                }
                s => return Err(invalid(format!("run {id} is {}, not running", s.as_str()))),
            }
            let now = self.now();
            if let Some(open) = open_step_in(tx, id)? {
                tx.execute(
                    "UPDATE bot_steps SET kind = ?3, attempt = attempt + 1
                      WHERE run_id = ?1 AND n = ?2",
                    params![id, open.n, kind.as_str()],
                )?;
                return Ok(Step {
                    kind,
                    attempt: open.attempt + 1,
                    ..open
                });
            }
            tx.execute(
                "INSERT INTO bot_steps (run_id, n, kind, started_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, n, kind.as_str(), now],
            )?;
            Ok(Step {
                run_id: id.into(),
                n,
                kind,
                started_at: now,
                attempt: 1,
                committed_at: None,
                payload: None,
                observed: Vec::new(),
                failures: 0,
            })
        })
    }

    /// Commits step `n` of run `id` with `commit` for `driver`, which holds
    /// the run's claim, in one transaction (see the module documentation),
    /// events recorded under `ctx`. A terminal step ends the run and
    /// records `bot.run_end {run, outcome, reason, credits_milli}` with the
    /// run's total. This is the only way usage is charged.
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` when `driver` does not hold the run
    /// (another driver took it over); `run_invalid` when step `n` is not
    /// the run's open step; the charge's (`unknown_cost_line`,
    /// `invalid_usage`), which commit nothing; the store's.
    pub fn commit_bot_step(
        &self,
        ctx: &AuditCtx,
        id: &str,
        driver: &DriverId,
        n: i64,
        commit: StepCommit,
    ) -> Result<Committed> {
        let driver = driver.as_str();
        let at = self.now_utc();
        let ov = self.bot_overrides();
        self.with_tx(|tx| {
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver)?;
            let open = open_step_in(tx, id)?
                .filter(|s| s.n == n)
                .ok_or_else(|| invalid(format!("step {n} of run {id} is not begun and uncommitted")))?;
            if run.status.is_terminal() {
                return Err(invalid(format!("run {id} has ended")));
            }
            let usage = charge_in(tx, &ov, &run.person, Some((id, n)), &commit.usage, at)?;
            let credits: i64 = usage.iter().map(|u| u.credits_milli).sum();
            for d in &commit.draft {
                match d {
                    DraftChange::Write {
                        path,
                        content,
                        content_type,
                    } => tx.execute(
                        "INSERT INTO bot_draft_files (run_id, path, content, content_type)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (run_id, path) DO UPDATE SET content = excluded.content,
                           content_type = excluded.content_type",
                        params![id, path, content, content_type],
                    )?,
                    DraftChange::Delete { path } => tx.execute(
                        "INSERT INTO bot_draft_files (run_id, path, content, content_type)
                         VALUES (?1, ?2, NULL, NULL)
                         ON CONFLICT (run_id, path) DO UPDATE SET content = NULL, content_type = NULL",
                        params![id, path],
                    )?,
                };
            }
            let now = clax_core::time::stamp(at);
            tx.execute(
                "UPDATE bot_steps SET committed_at = ?3, payload_json = ?4 WHERE run_id = ?1 AND n = ?2",
                params![id, n, now, commit.payload.to_string()],
            )?;
            let total = run.credits_milli + credits;
            let end = open.kind.ends_as();
            match end {
                Some(status) => tx.execute(
                    "UPDATE bot_runs SET credits_milli = ?2, status = ?3, reason = ?4, updated_at = ?5,
                       ended_at = ?5, claimed_by = NULL, claimed_at = NULL WHERE id = ?1",
                    params![id, total, status.as_str(), commit.reason, now],
                )?,
                None => tx.execute(
                    "UPDATE bot_runs SET credits_milli = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, total, now],
                )?,
            };
            for mut rec in commit.events {
                rec.ids.artifact = Some(run.artifact_id.clone());
                rec.body.insert("run".into(), id.into());
                rec.body.insert("step".into(), n.into());
                if matches!(rec.kind, AuditKind::BotTurn | AuditKind::BotScript) {
                    rec.body.entry("credits_milli".into()).or_insert(credits.into());
                }
                self.record_audit(tx, ctx, rec)?;
            }
            if let Some(status) = end {
                let mut rec = AuditRecord::new(AuditKind::BotRunEnd, now.clone())
                    .with("run", id)
                    .with("outcome", status.as_str())
                    .with("reason", commit.reason.clone())
                    .with("credits_milli", total);
                rec.ids.artifact = Some(run.artifact_id.clone());
                self.record_audit(tx, ctx, rec)?;
            }
            let step = Step {
                committed_at: Some(now),
                payload: Some(commit.payload),
                ..open
            };
            Ok(Committed {
                step,
                usage,
                credits_milli: credits,
                run: run_in(tx, id)?.expect("the run exists"),
            })
        })
    }

    /// Asks run `id` to stop, as `ctx`: a queued or running run becomes
    /// `stopping`, recorded as `bot.cancel {run, by}` (`by` the person key
    /// of who asked, `*system` for Clax itself), and its driver writes its
    /// `stopped` step at the next boundary. Returns the run after; an ended
    /// or stopping run is unchanged and records nothing.
    ///
    /// # Errors
    /// `NotFound`; the store's.
    pub fn cancel_run(&self, ctx: &AuditCtx, id: &str) -> Result<Run> {
        self.with_tx(|tx| {
            let now = self.now();
            let changed = tx.execute(
                "UPDATE bot_runs SET status = 'stopping', updated_at = ?2
                  WHERE id = ?1 AND status IN ('queued', 'running')",
                params![id, now],
            )?;
            let run = run_in(tx, id)?.ok_or(CoreError::NotFound)?;
            if changed > 0 {
                let mut rec = AuditRecord::new(AuditKind::BotCancel, now)
                    .with("run", id)
                    .with("by", set_by(ctx));
                rec.ids.artifact = Some(run.artifact_id.clone());
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(run)
        })
    }

    /// Records `entry` as what attempt `attempt` of run `id`'s open step
    /// `n` has used so far (replacing that attempt's earlier entry), for
    /// `driver`, which holds the run: a model call's usage as it streams,
    /// so an attempt that never commits is still accounted for. Returns
    /// whether it was recorded (`false` once the driver lost the run or the
    /// step moved on).
    ///
    /// # Errors
    /// The store's.
    pub fn observe_step(
        &self,
        id: &str,
        driver: &DriverId,
        n: i64,
        attempt: i64,
        entry: Value,
    ) -> Result<bool> {
        let driver = driver.as_str();
        self.with_tx(|tx| {
            let Some(run) = run_in(tx, id)? else {
                return Ok(false);
            };
            if run.claimed_by.as_deref() != Some(driver) {
                return Ok(false);
            }
            let Some(open) = open_step_in(tx, id)?.filter(|s| s.n == n) else {
                return Ok(false);
            };
            let mut observed = open.observed;
            let mut entry = entry;
            entry["attempt"] = attempt.into();
            match observed.iter_mut().find(|e| e["attempt"] == attempt) {
                Some(e) => *e = entry,
                None => observed.push(entry),
            }
            tx.execute(
                "UPDATE bot_steps SET observed_json = ?3 WHERE run_id = ?1 AND n = ?2",
                params![id, n, Value::Array(observed).to_string()],
            )?;
            Ok(true)
        })
    }

    /// Counts one more failed commit of run `id`'s open step `n`; returns
    /// how many there have been (0 when `n` is not the open step).
    ///
    /// # Errors
    /// The store's.
    pub fn note_commit_failure(&self, id: &str, n: i64) -> Result<i64> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_steps SET failures = failures + 1
                  WHERE run_id = ?1 AND n = ?2 AND committed_at IS NULL",
                params![id, n],
            )?;
            Ok(tx
                .query_row(
                    "SELECT failures FROM bot_steps WHERE run_id = ?1 AND n = ?2 AND committed_at IS NULL",
                    params![id, n],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    /// The journal of run `id`'s step `n`: every call its scripts made, by
    /// `seq`.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_calls(&self, id: &str, n: i64) -> Result<Vec<CallEntry>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {CALL_COLS} FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 ORDER BY seq"
            ))?;
            let rows = q.query_map(params![id, n], row_to_call)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The journal entry of call `seq` (`op` on arguments hashing to
    /// `args_hash`) of attempt `attempt` of step `step_n` of run `run_id`:
    /// made only for the driver holding the run, so no one else can
    /// journal under a key of its own.
    ///
    /// # Errors
    /// `run_not_claimed` unless `driver` holds the run; `NotFound`; the
    /// store's.
    #[allow(clippy::too_many_arguments)]
    pub fn call_entry(
        &self,
        run_id: &str,
        driver: &DriverId,
        step_n: i64,
        attempt: i64,
        seq: i64,
        op: &str,
        args_hash: &str,
    ) -> Result<CallEntry> {
        let run = self.run(run_id)?.ok_or(CoreError::NotFound)?;
        check_claim(&run, driver.as_str())?;
        Ok(CallEntry {
            run_id: run_id.to_string(),
            step_n,
            seq,
            attempt,
            op: op.to_string(),
            args_hash: args_hash.to_string(),
            ok: true,
            result: Value::Null,
        })
    }

    /// Journals `entry` unless its `(run, step, seq)` already is; returns the
    /// entry the journal holds for it (the earlier one when there was one).
    ///
    /// # Errors
    /// The store's.
    pub fn record_bot_call(&self, entry: &CallEntry) -> Result<CallEntry> {
        self.with_tx(|tx| {
            if let Some(prev) = call_in(tx, entry)? {
                return Ok(prev);
            }
            insert_call(tx, entry, &self.now())?;
            Ok(entry.clone())
        })
    }

    /// Performs a write with its journal entry in one transaction: when the
    /// journal already holds `entry`'s `(run, step, seq)`, answers with what
    /// it recorded (the same op and arguments) or `key_reused` (others),
    /// performing nothing; else runs `write` on the transaction and
    /// journals its value, both committed together, or neither when either
    /// fails.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure as `Invalid` with its code; `write`'s;
    /// the store's.
    pub fn with_bot_journal(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
        write: impl FnOnce(&Tx<'_>) -> Result<Value>,
    ) -> Result<Journaled> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(prev);
        }
        self.with_tx(|tx| {
            guard(&Tx(tx))?;
            let value = write(&Tx(tx))?;
            self.journal_in(tx, entry, value)
        })
    }

    /// The first half of a write to an artifact's own store journaled as
    /// `entry` (ClaxBot's `db.set`, `db.update` and `db.delete`): the
    /// journal's answer when it holds `entry`'s key (the caller writes
    /// nothing), else `None` once `guard` passed in a transaction of its
    /// own. The caller then writes under [`CallEntry::op_id`], which the
    /// artifact's store keeps with the write, and journals it
    /// ([`Store::journal_app_write`]). There is no transaction across the
    /// two stores: a crash between the write and its entry leaves the
    /// write, and the call made again writes nothing more and journals
    /// what the write answered.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure; `guard`'s; the store's.
    pub fn app_write_begin(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
    ) -> Result<Option<Journaled>> {
        if let Some(prev) = self.journaled(entry)? {
            return Ok(Some(prev));
        }
        self.with_tx(|tx| guard(&Tx(tx)))?;
        Ok(None)
    }

    /// The second half of [`Store::app_write_begin`]: journals `value`, the
    /// answer of the write the artifact's store made under `entry`'s
    /// operation ID, after `guard` passes again, in one transaction. A
    /// person who lost the right between the write and this call is refused
    /// here, and the write stands without its entry.
    ///
    /// # Errors
    /// `guard`'s; the store's.
    pub fn journal_app_write(
        &self,
        entry: &CallEntry,
        guard: Guard<'_>,
        value: Value,
    ) -> Result<Journaled> {
        self.with_tx(|tx| {
            guard(&Tx(tx))?;
            self.journal_in(tx, entry, value)
        })
    }

    /// The journal's answer for `entry`'s key, when it holds one: what a
    /// journaled write answers without writing.
    ///
    /// # Errors
    /// `key_reused`; a recorded failure; the store's.
    pub(crate) fn journaled(&self, entry: &CallEntry) -> Result<Option<Journaled>> {
        let prev = self.with_read(|c| call_in(c, entry))?;
        prev.map(|p| answer_of(&p, entry).map(Journaled))
            .transpose()
    }

    /// Journals `value` as `entry`'s answer on `tx`, the write's own
    /// transaction (a failure rolls the write back with it).
    pub(crate) fn journal_in(
        &self,
        tx: &Connection,
        entry: &CallEntry,
        value: Value,
    ) -> Result<Journaled> {
        let fault = self
            .journal_fault
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if fault.is_some_and(|f| f(entry)) {
            return Err(CoreError::invalid(
                "journal_fault",
                "a crash was injected between the write and its journal entry",
            ));
        }
        let done = CallEntry {
            ok: true,
            result: value.clone(),
            ..entry.clone()
        };
        insert_call(tx, &done, &self.now())?;
        Ok(Journaled(value))
    }

    /// Makes every journaled write whose entry `fault` accepts fail after
    /// the write and before its journal row, rolling both back, as a crash
    /// there would; `None` clears it. For crash tests
    /// ([`clax-bot`'s host conformance suite]).
    #[doc(hidden)]
    pub fn set_journal_fault(&self, fault: Option<JournalFault>) {
        *self
            .journal_fault
            .write()
            .unwrap_or_else(|e| e.into_inner()) = fault;
    }

    /// Run `id`'s draft overlay, by path: each path it wrote or deleted.
    ///
    /// # Errors
    /// The store's.
    pub fn draft_files(&self, id: &str) -> Result<Vec<DraftFile>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT path, content, content_type FROM bot_draft_files WHERE run_id = ?1 ORDER BY path",
            )?;
            let rows = q.query_map(params![id], |r| {
                Ok(DraftFile {
                    path: r.get(0)?,
                    content: r.get(1)?,
                    content_type: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Removes what ended runs no longer need, by the store's clock: the
    /// call journal (`bot_calls`) of each run that ended over
    /// [`BOT_JOURNAL_KEEP`] ago, and the draft overlay (`bot_draft_files`)
    /// of each that ended over [`BOT_DRAFT_KEEP`] ago. The runs, their
    /// steps, usage and audit events stay. How many rows of each it
    /// removed.
    ///
    /// # Errors
    /// The store's.
    pub fn prune_ended_runs(&self) -> Result<Pruned> {
        let now = self.now_utc();
        let before = |keep: std::time::Duration| {
            clax_core::time::stamp(
                now - chrono::Duration::from_std(keep).unwrap_or(chrono::Duration::MAX),
            )
        };
        let (journal, draft) = (before(BOT_JOURNAL_KEEP), before(BOT_DRAFT_KEEP));
        self.with_tx(|tx| {
            let calls = tx.execute(
                "DELETE FROM bot_calls WHERE run_id IN
                   (SELECT id FROM bot_runs WHERE ended_at IS NOT NULL AND ended_at < ?1)",
                params![journal],
            )?;
            let draft_files = tx.execute(
                "DELETE FROM bot_draft_files WHERE run_id IN
                   (SELECT id FROM bot_runs WHERE ended_at IS NOT NULL AND ended_at < ?1)",
                params![draft],
            )?;
            Ok(Pruned { calls, draft_files })
        })
    }
}

/// How long an ended run's call journal is kept: the journal answers a
/// step's calls made again, so it outlives any driver that lost the run
/// and may still be running one of its steps (a run's wall time is capped
/// far below this).
pub const BOT_JOURNAL_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// How long an ended run's draft is kept, viewable from its run (spec
/// 2026-10-21-claxbot-design §6.3: a run stopped on its limits keeps its
/// draft).
pub const BOT_DRAFT_KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// What [`Store::prune_ended_runs`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Call journal rows.
    pub calls: usize,
    /// Draft overlay rows.
    pub draft_files: usize,
}

/// A store transaction a journaled write runs on ([`Store::with_bot_journal`]);
/// opaque outside the store, which keeps it for the `_in` write paths a
/// journaled write may call.
pub struct Tx<'a>(#[allow(dead_code)] pub(crate) &'a Connection);

/// A write's answer as its call journal recorded it: made only by the
/// store's journaled write paths, which record the entry in the write's
/// own transaction ([`Store::with_bot_journal`],
/// [`Store::add_comment_journaled`], [`Store::resolve_thread_journaled`],
/// [`Store::doc_set_journaled`] and its siblings,
/// [`Store::publish_version_journaled`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Journaled(pub(crate) Value);

impl Journaled {
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }

    #[must_use]
    pub fn into_value(self) -> Value {
        self.0
    }
}

/// A test hook that fails a journaled write after the write
/// ([`Store::set_journal_fault`]).
pub type JournalFault = std::sync::Arc<dyn Fn(&CallEntry) -> bool + Send + Sync>;

/// One entry of a step's call journal: a script's call `seq` (from 0)
/// made in attempt `attempt`, its op and the SHA-256 of its arguments, and
/// what it answered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallEntry {
    pub(crate) run_id: String,
    pub(crate) step_n: i64,
    pub(crate) seq: i64,
    pub(crate) attempt: i64,
    pub(crate) op: String,
    pub(crate) args_hash: String,
    pub(crate) ok: bool,
    /// The value, or `{code, message}` for a failure.
    pub(crate) result: Value,
}

impl CallEntry {
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The operation ID a write to an artifact's own store carries for
    /// this call ([`Store::app_write_begin`]): its place, the same on every
    /// attempt of the step, and its op and arguments as the digest.
    #[must_use]
    pub fn op_id(&self) -> crate::app_data::OpId {
        crate::app_data::OpId {
            id: format!("bot:{}:{}:{}", self.run_id, self.step_n, self.seq),
            digest: format!("{}:{}", self.op, self.args_hash),
        }
    }

    #[must_use]
    pub fn step_n(&self) -> i64 {
        self.step_n
    }

    #[must_use]
    pub fn seq(&self) -> i64 {
        self.seq
    }

    #[must_use]
    pub fn attempt(&self) -> i64 {
        self.attempt
    }

    #[must_use]
    pub fn op(&self) -> &str {
        &self.op
    }

    #[must_use]
    pub fn args_hash(&self) -> &str {
        &self.args_hash
    }

    #[must_use]
    pub fn ok(&self) -> bool {
        self.ok
    }

    #[must_use]
    pub fn result(&self) -> &Value {
        &self.result
    }

    /// The same call, answered `result` (`ok`, or a failure's `{code,
    /// message}`), for [`Store::record_bot_call`].
    #[must_use]
    pub fn answered(self, ok: bool, result: Value) -> CallEntry {
        CallEntry { ok, result, ..self }
    }
}

/// Run inside a journaled write's transaction before it commits (the
/// acting person may still write here, the run still acts): an error
/// rolls the write and its entry back.
pub type Guard<'a> = &'a (dyn Fn(&Tx<'_>) -> Result<()> + Send + Sync);

const CALL_COLS: &str = "run_id, step_n, seq, attempt, op, args_hash, ok, result_json";

fn row_to_call(r: &Row<'_>) -> rusqlite::Result<CallEntry> {
    let result: String = r.get(7)?;
    Ok(CallEntry {
        run_id: r.get(0)?,
        step_n: r.get(1)?,
        seq: r.get(2)?,
        attempt: r.get(3)?,
        op: r.get(4)?,
        args_hash: r.get(5)?,
        ok: r.get(6)?,
        result: serde_json::from_str(&result).unwrap_or(Value::Null),
    })
}

pub(crate) fn call_in(c: &Connection, e: &CallEntry) -> Result<Option<CallEntry>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {CALL_COLS} FROM bot_calls WHERE run_id = ?1 AND step_n = ?2 AND seq = ?3"
    ))?
    .query_row(params![e.run_id, e.step_n, e.seq], row_to_call)
    .optional()?)
}

pub(crate) fn insert_call(c: &Connection, e: &CallEntry, at: &str) -> Result<()> {
    c.execute(
        "INSERT INTO bot_calls (run_id, step_n, seq, attempt, op, args_hash, ok, result_json, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            e.run_id,
            e.step_n,
            e.seq,
            e.attempt,
            e.op,
            e.args_hash,
            e.ok,
            e.result.to_string(),
            at
        ],
    )?;
    Ok(())
}

/// The answer the journal's `prev` gives a call `e` at its key.
pub(crate) fn answer_of(prev: &CallEntry, e: &CallEntry) -> Result<Value> {
    if prev.op != e.op || prev.args_hash != e.args_hash {
        return Err(CoreError::invalid(
            "key_reused",
            format!(
                "call {} of step {} of run {} was {} with other arguments",
                e.seq, e.step_n, e.run_id, prev.op
            ),
        ));
    }
    if prev.ok {
        Ok(prev.result.clone())
    } else {
        let code = match prev.result["code"].as_str() {
            Some("forbidden") => "forbidden",
            Some("conflict") => "conflict",
            Some("not_found") => "not_found",
            _ => "call_failed",
        };
        Err(CoreError::invalid(
            code,
            prev.result["message"].as_str().unwrap_or("").to_string(),
        ))
    }
}

/// A `bot.turn` record for a step commit: the model, the key it ran on,
/// the token counts and why the turn stopped (`credits_milli`, `run` and
/// `step` are filled at commit).
pub fn turn_record(
    at: impl Into<String>,
    model: &str,
    key_source: KeySource,
    input_tokens: i64,
    cached_tokens: i64,
    output_tokens: i64,
    stop_reason: &str,
) -> AuditRecord {
    AuditRecord::new(AuditKind::BotTurn, at)
        .with("model", model)
        .with("key_source", key_source.as_str())
        .with("input_tokens", input_tokens)
        .with("cached_tokens", cached_tokens)
        .with("output_tokens", output_tokens)
        .with("stop_reason", stop_reason)
}

/// A `bot.script` record: the script's title, the SHA-256 of its code
/// (the code itself is in the step's payload), the ops it called, its CPU
/// time, peak memory and error.
pub fn script_record(
    at: impl Into<String>,
    title: &str,
    code: &str,
    calls: &[String],
    cpu_ms: i64,
    mem_peak: i64,
    error: Option<&str>,
) -> AuditRecord {
    AuditRecord::new(AuditKind::BotScript, at)
        .with("title", title)
        .with(
            "code_sha256",
            format!("sha256:{}", clax_core::audit::sha256_hex(code.as_bytes())),
        )
        .with("calls", calls.to_vec())
        .with("cpu_ms", cpu_ms)
        .with("mem_peak", mem_peak)
        .with("error", error)
}

/// A `bot.call` record: one API op a script called, what it named, and
/// whether it was allowed.
pub fn call_record(at: impl Into<String>, op: &str, target: Option<&str>, ok: bool) -> AuditRecord {
    AuditRecord::new(AuditKind::BotCall, at)
        .with("op", op)
        .with("target", target)
        .with("ok", ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credits::{LINE_HARNESS_FEE, Unit, token_line};
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store, store_with_clock};
    use clax_core::audit::Actor;
    use clax_core::working::ManualClock;
    use serde_json::json;
    use std::sync::Arc;

    fn bot() -> AuditCtx {
        AuditCtx {
            actor: Actor::Bot {
                on_behalf_of: OWNER.into(),
            },
            ..DAEMON.clone()
        }
    }

    fn new_run_on(st: &Store, model: &str) -> Result<Run> {
        let aid = artifact(st, None).to_string();
        st.create_run(
            &bot(),
            NewRun {
                artifact_id: aid,
                person: OWNER.into(),
                request: RequestKind::Thread,
                thread_ids: vec!["t1".into()],
                model: model.into(),
                key_source: KeySource::Instance,
                base_version: 1,
                agent_grant: None,
            },
        )
    }

    fn new_run(st: &Store) -> Run {
        new_run_on(st, "claude-sonnet-5-5").unwrap()
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_run_is_claimed_once_and_resumes_its_uncommitted_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        assert_eq!(run.status, RunStatus::Queued);
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)),
            "run_not_claimed"
        );
        let held = st
            .claim_run(&run.id, &DriverId::named("d1"), 60)
            .unwrap()
            .unwrap();
        assert_eq!(held.status, RunStatus::Running);
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_none(),
            "held by d1"
        );
        assert!(
            st.claim_run(&run.id, &DriverId::named("d1"), 60)
                .unwrap()
                .is_some(),
            "renewed"
        );
        assert_eq!(st.live_runs().unwrap().len(), 1);
        assert_eq!(st.next_step_n(&run.id).unwrap(), 1);

        let s1 = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)
            .unwrap();
        assert_eq!(s1.n, 1);
        // The driver dies here. On restart the step is found again.
        st.release_run(&run.id, &DriverId::named("d1")).unwrap();
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_some()
        );
        let open = st.open_step(&run.id).unwrap().unwrap();
        assert_eq!((open.n, open.committed_at.clone()), (1, None));
        assert_eq!(st.next_step_n(&run.id).unwrap(), 1);
        let again = st
            .begin_step(&run.id, &DriverId::named("d2"), 1, StepKind::Turn)
            .unwrap();
        assert_eq!(again.n, 1, "the open step, not a new one");
        assert_eq!((s1.attempt, again.attempt), (1, 2), "begun again");
        st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("d2"),
            1,
            StepCommit {
                payload: json!({"messages": ["hi"]}),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(st.open_step(&run.id).unwrap().is_none());
        // A driver that lost track (it thinks step 1 is still open) is
        // refused, and so is one that skips ahead.
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d2"), 1, StepKind::Turn)),
            "step_mismatch"
        );
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d2"), 3, StepKind::Turn)),
            "step_mismatch"
        );
        assert_eq!(
            st.begin_step(&run.id, &DriverId::named("d2"), 2, StepKind::Script)
                .unwrap()
                .n,
            2
        );
        assert_eq!(
            code(st.commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d2"),
                1,
                StepCommit::default()
            )),
            "run_invalid",
            "committed"
        );
    }

    /// An ended run's call journal goes a day after it ended and its draft
    /// thirty days after; a run still going keeps both.
    #[test]
    fn ended_runs_lose_their_journal_then_their_draft() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let d = DriverId::named("d1");
        let work = |run: &Run, kind: StepKind| {
            st.claim_run(&run.id, &d, 60).unwrap().unwrap();
            let n = st.next_step_n(&run.id).unwrap();
            st.begin_step(&run.id, &d, n, kind).unwrap();
            let entry = st
                .call_entry(&run.id, &d, n, 1, 0, "threads.read", "h")
                .unwrap();
            st.record_bot_call(&entry).unwrap();
            st.commit_bot_step(
                &bot(),
                &run.id,
                &d,
                n,
                StepCommit {
                    draft: vec![DraftChange::Write {
                        path: "index.html".into(),
                        content: b"<p>".to_vec(),
                        content_type: None,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        };
        let (ended, going) = (new_run(&st), new_run(&st));
        work(&ended, StepKind::Turn);
        work(&ended, StepKind::GiveUp);
        work(&going, StepKind::Turn);
        let calls = |run: &Run| st.bot_calls(&run.id, 1).unwrap().len();
        let drafts = |run: &Run| st.draft_files(&run.id).unwrap().len();
        assert_eq!(st.prune_ended_runs().unwrap(), Pruned::default());

        clock.advance(BOT_JOURNAL_KEEP.as_secs() as i64 + 1);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.draft_files), (2, 0));
        assert_eq!((calls(&ended), drafts(&ended)), (0, 1));
        assert_eq!((calls(&going), drafts(&going)), (1, 1));

        clock.advance(BOT_DRAFT_KEEP.as_secs() as i64);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.draft_files), (0, 1));
        assert_eq!(drafts(&ended), 0);
        assert_eq!((calls(&going), drafts(&going)), (1, 1));
        assert_eq!(st.run(&ended.id).unwrap().unwrap().status, RunStatus::GaveUp);
    }

    #[test]
    fn a_driver_whose_lease_lapsed_cannot_commit_over_the_one_that_took_over() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("a"), 60)
            .unwrap()
            .unwrap();
        st.begin_step(&run.id, &DriverId::named("a"), 1, StepKind::Turn)
            .unwrap();
        // A long turn renews its lease and keeps the run.
        clock.advance(50);
        assert!(st.renew_lease(&run.id, &DriverId::named("a")).unwrap());
        clock.advance(50);
        assert!(
            st.claim_run(&run.id, &DriverId::named("b"), 60)
                .unwrap()
                .is_none(),
            "renewed 50 s ago"
        );
        // A stalled driver's lease lapses; another takes the run over and
        // runs the open step again.
        clock.advance(11);
        assert!(
            st.claim_run(&run.id, &DriverId::named("b"), 60)
                .unwrap()
                .is_some()
        );
        assert!(
            !st.renew_lease(&run.id, &DriverId::named("a")).unwrap(),
            "a has lost it"
        );
        st.begin_step(&run.id, &DriverId::named("b"), 1, StepKind::Turn)
            .unwrap();
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        let late = st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("a"),
            1,
            StepCommit {
                payload: json!({"by": "a"}),
                usage: vec![Metered::new(&line, 1_000, KeySource::Instance)],
                ..Default::default()
            },
        );
        assert_eq!(code(late), "run_not_claimed");
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("a"), 1, StepKind::Turn)),
            "run_not_claimed"
        );
        let won = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("b"),
                1,
                StepCommit {
                    payload: json!({"by": "b"}),
                    usage: vec![Metered::new(&line, 1_000, KeySource::Instance)],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(won.step.payload, Some(json!({"by": "b"})));
        assert_eq!(won.usage.len(), 1, "charged once");
        assert_eq!(st.usage_rows(OWNER, st.now_utc()).unwrap().len(), 1);
    }

    #[test]
    fn a_model_without_prices_cannot_run() {
        let (_d, st) = store();
        assert_eq!(code(new_run_on(&st, "custom-model")), "model_not_priced");
        // A model whose lines go away while a run waits is refused at its
        // next turn, before anything is asked of the provider.
        let run = new_run_on(&st, "claude-opus-5-5").unwrap();
        st.with_write(|c| {
            c.execute(
                "DELETE FROM cost_lines WHERE id = 'claude-opus-5-5.output'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        st.claim_run(&run.id, &DriverId::named("d"), 60).unwrap();
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d"), 1, StepKind::Turn)),
            "model_not_priced"
        );
        // Its stop is still possible.
        st.cancel_run(DAEMON, &run.id).unwrap();
        st.begin_step(&run.id, &DriverId::named("d"), 1, StepKind::Stopped)
            .unwrap();
    }

    #[test]
    fn a_step_commits_its_usage_draft_and_events_together() {
        let (_d, st) = store();
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d1"), 60).unwrap();
        let s = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)
            .unwrap();
        let line = token_line("claude-sonnet-5-5", Unit::MtokInput);
        let turn = |n: i64| {
            turn_record(
                st.now(),
                "claude-sonnet-5-5",
                KeySource::Instance,
                n,
                0,
                10,
                "tool_use",
            )
        };
        // A charge on an unknown line refuses the whole commit.
        let bad = st.commit_bot_step(
            &bot(),
            &run.id,
            &DriverId::named("d1"),
            s.n,
            StepCommit {
                payload: json!({}),
                usage: vec![Metered::new("nope", 1, KeySource::Instance)],
                events: vec![turn(1)],
                draft: vec![DraftChange::Delete {
                    path: "a.css".into(),
                }],
                reason: None,
            },
        );
        assert!(bad.is_err());
        assert!(st.open_step(&run.id).unwrap().is_some());
        assert!(st.draft_files(&run.id).unwrap().is_empty());
        assert!(
            st.events_after(0, 1000)
                .unwrap()
                .iter()
                .all(|e| e.kind != "bot.turn")
        );
        assert!(st.usage_rows(OWNER, st.now_utc()).unwrap().is_empty());

        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d1"),
                s.n,
                StepCommit {
                    payload: json!({"messages": []}),
                    usage: vec![Metered::new(&line, 1_000_000, KeySource::Instance)],
                    events: vec![turn(1_000_000)],
                    draft: vec![
                        DraftChange::Write {
                            path: "index.html".into(),
                            content: b"<p>new</p>".to_vec(),
                            content_type: Some("text/html".into()),
                        },
                        DraftChange::Delete {
                            path: "old.css".into(),
                        },
                    ],
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(done.credits_milli, 280_000);
        assert_eq!(done.run.credits_milli, 280_000);
        assert_eq!(done.usage[0].run_id.as_deref(), Some(run.id.as_str()));
        let draft = st.draft_files(&run.id).unwrap();
        assert_eq!(draft.len(), 2);
        assert_eq!(draft[0].content.as_deref(), Some(&b"<p>new</p>"[..]));
        assert_eq!(draft[1].content, None);
        let turn = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == "bot.turn")
            .unwrap();
        let body: Value = serde_json::from_str(&turn.body).unwrap();
        assert_eq!(body["credits_milli"], 280_000);
        assert_eq!(body["key_source"], "instance");
        assert_eq!(body["run"], run.id.as_str());
        assert_eq!(body["step"], 1);
        assert_eq!(turn.ids.artifact.as_deref(), Some(run.artifact_id.as_str()));
        assert!(turn.actor.contains("\"type\":\"bot\""));
    }

    #[test]
    fn cancel_is_recorded_and_stops_at_the_next_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d1"), 60).unwrap();
        let sam = AuditCtx {
            actor: Actor::Viewer {
                public_id: "u_sam".into(),
                display_name: None,
                person: true,
            },
            ..DAEMON.clone()
        };
        assert_eq!(
            st.cancel_run(&sam, &run.id).unwrap().status,
            RunStatus::Stopping
        );
        // Asking again changes nothing and records nothing.
        st.cancel_run(&sam, &run.id).unwrap();
        assert_eq!(
            code(st.begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Turn)),
            "run_invalid"
        );
        let s = st
            .begin_step(&run.id, &DriverId::named("d1"), 1, StepKind::Stopped)
            .unwrap();
        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &DriverId::named("d1"),
                s.n,
                StepCommit {
                    payload: json!({}),
                    usage: vec![Metered::new(LINE_HARNESS_FEE, 1, KeySource::Instance)],
                    reason: Some("cancelled".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(done.run.status, RunStatus::Stopped);
        assert_eq!(done.run.reason.as_deref(), Some("cancelled"));
        assert!(done.run.ended_at.is_some() && done.run.claimed_by.is_none());
        assert!(st.live_runs().unwrap().is_empty());
        assert_eq!(
            st.cancel_run(&sam, &run.id).unwrap().status,
            RunStatus::Stopped
        );
        assert!(
            st.claim_run(&run.id, &DriverId::named("d1"), 60)
                .unwrap()
                .is_none()
        );
        let events: Vec<(String, String)> = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("bot."))
            .map(|e| (e.kind, e.body))
            .collect();
        let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["bot.run_start", "bot.cancel", "bot.run_end"]);
        assert!(events[1].1.contains("\"by\":\"u_sam\""), "{}", events[1].1);
        let (r, steps) = st.run_with_steps(&run.id).unwrap().unwrap();
        assert_eq!(r.credits_milli, 1_000);
        assert_eq!(steps.len(), 1);
        assert_eq!(st.runs_for_artifact(&run.artifact_id, 10).unwrap().len(), 1);
        assert_eq!(st.runs_for_person(OWNER, 10).unwrap()[0].id, run.id);
        assert!(st.runs_for_person("u_other", 10).unwrap().is_empty());
    }

    #[test]
    fn a_stale_claim_is_taken_over() {
        let clock = Arc::new(ManualClock::at("2026-10-21T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let run = new_run(&st);
        st.claim_run(&run.id, &DriverId::named("d0"), 60)
            .unwrap()
            .unwrap();
        // A lease of 0 takes a claim at once, as a restarted daemon does.
        st.claim_run(&run.id, &DriverId::named("d1"), 0)
            .unwrap()
            .unwrap();
        clock.advance(30);
        assert!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .is_none()
        );
        clock.advance(31);
        assert_eq!(
            st.claim_run(&run.id, &DriverId::named("d2"), 60)
                .unwrap()
                .unwrap()
                .claimed_by
                .as_deref(),
            Some("d2")
        );
    }

    #[test]
    fn observed_usage_and_commit_failures_are_kept_with_the_open_step() {
        let (_d, st) = store();
        let run = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Turn).unwrap();
        assert!(
            st.observe_step(&run.id, &d, 1, 1, json!({"input_tokens": 5}))
                .unwrap()
        );
        assert!(
            st.observe_step(&run.id, &d, 1, 1, json!({"input_tokens": 7}))
                .unwrap()
        );
        assert_eq!(
            st.begin_step(&run.id, &d, 1, StepKind::Turn)
                .unwrap()
                .attempt,
            2
        );
        assert!(
            st.observe_step(&run.id, &d, 1, 2, json!({"input_tokens": 9}))
                .unwrap()
        );
        assert!(
            !st.observe_step(&run.id, &DriverId::named("d2"), 1, 2, json!({}))
                .unwrap(),
            "not the claim holder"
        );
        assert!(
            !st.observe_step(&run.id, &d, 2, 1, json!({})).unwrap(),
            "not the open step"
        );
        let open = st.open_step(&run.id).unwrap().unwrap();
        assert_eq!(open.observed.len(), 2);
        assert_eq!(open.observed[0], json!({"input_tokens": 7, "attempt": 1}));
        assert_eq!(st.note_commit_failure(&run.id, 1).unwrap(), 1);
        assert_eq!(st.note_commit_failure(&run.id, 1).unwrap(), 2);
        assert_eq!(st.note_commit_failure(&run.id, 9).unwrap(), 0);
        assert_eq!(st.open_step(&run.id).unwrap().unwrap().failures, 2);
    }

    #[test]
    fn the_call_journal_answers_a_repeated_key_and_refuses_a_reused_one() {
        let (_d, st) = store();
        let run = new_run(&st);
        let entry = CallEntry {
            run_id: run.id.clone(),
            step_n: 1,
            seq: 0,
            attempt: 1,
            op: "threads.reply".into(),
            args_hash: "h1".into(),
            ok: true,
            result: Value::Null,
        };
        let mut writes = 0;
        let v = st
            .with_bot_journal(&entry, &|_| Ok(()), |_| {
                writes += 1;
                Ok(json!({"comment_id": "c1"}))
            })
            .unwrap()
            .into_value();
        let again = st
            .with_bot_journal(
                &CallEntry {
                    attempt: 2,
                    ..entry.clone()
                },
                &|_| Ok(()),
                |_| {
                    writes += 1;
                    Ok(json!({"comment_id": "c2"}))
                },
            )
            .unwrap()
            .into_value();
        assert_eq!(
            (v.clone(), again, writes),
            (json!({"comment_id": "c1"}), v, 1)
        );
        let other = CallEntry {
            args_hash: "h2".into(),
            ..entry.clone()
        };
        assert_eq!(
            code(st.with_bot_journal(&other, &|_| Ok(()), |_| Ok(Value::Null))),
            "key_reused"
        );
        let refused = CallEntry {
            seq: 1,
            ..entry.clone()
        };
        assert_eq!(
            code(
                st.with_bot_journal(&refused, &|_| Ok(()), |_| Err(CoreError::invalid(
                    "forbidden",
                    "no"
                )))
            ),
            "forbidden"
        );
        let read = CallEntry {
            seq: 2,
            op: "threads.read".into(),
            ..entry.clone()
        };
        st.record_bot_call(&read).unwrap();
        let journal = st.bot_calls(&run.id, 1).unwrap();
        assert_eq!(journal.iter().map(|c| c.seq).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(journal[0].result, json!({"comment_id": "c1"}));
    }

    #[test]
    fn a_snapshot_rate_charges_as_then_even_without_its_line() {
        let (_d, st) = store();
        let run = new_run(&st);
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Turn).unwrap();
        let mut rate = st
            .rate_of(&token_line("claude-sonnet-5-5", Unit::MtokInput))
            .unwrap()
            .unwrap();
        rate.cost_micro_usd = 1_000_000;
        let mut m = Metered::absorbed("gone-model.input", 2_000_000, KeySource::Instance);
        m.rate = Some(rate);
        m.estimated = true;
        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &d,
                1,
                StepCommit {
                    payload: json!({}),
                    usage: vec![m],
                    ..Default::default()
                },
            )
            .unwrap();
        let row = &done.usage[0];
        assert_eq!(row.line, "gone-model.input");
        assert_eq!((row.cost_micro_usd, row.credits_milli), (2_000_000, 0));
        assert_eq!(row.rate_cost_micro_usd, 1_000_000);
        assert!(row.absorbed && row.estimated);
        assert_eq!(st.usage_rows(OWNER, st.now_utc()).unwrap()[0], *row);
    }

    #[test]
    fn a_journaled_comment_and_its_entry_commit_together_or_not_at_all() {
        let (_d, st) = store();
        let run = new_run(&st);
        let aid = clax_core::ArtifactId::parse(&run.artifact_id).unwrap();
        let t = st
            .create_thread(
                DAEMON,
                &aid,
                crate::threads::NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: crate::test_util::anchor(),
                    author_name: "Alex".into(),
                    body: "Make it blue.".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        let entry = CallEntry {
            run_id: run.id.clone(),
            step_n: 1,
            seq: 0,
            attempt: 1,
            op: "threads.reply".into(),
            args_hash: "h".into(),
            ok: true,
            result: Value::Null,
        };
        let reply = || crate::threads::NewComment {
            author_kind: clax_core::model::AUTHOR_AGENT,
            author_name: "ClaxBot".into(),
            author_public_id: None,
            via_session_id: None,
            body: "Done.".into(),
            via_page: false,
        };
        let comments = |st: &Store| st.get_thread(&t.id).unwrap().unwrap().comments.len();
        st.set_journal_fault(Some(std::sync::Arc::new(|e: &CallEntry| e.seq == 0)));
        assert_eq!(
            code(st.add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))),
            "journal_fault"
        );
        assert_eq!(comments(&st), 1, "the reply rolled back with its entry");
        assert!(st.bot_calls(&run.id, 1).unwrap().is_empty());
        st.set_journal_fault(None);
        let first = st
            .add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))
            .unwrap();
        let again = st
            .add_comment_journaled(&bot(), &t.id, reply(), &entry, &|_| Ok(()))
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(comments(&st), 2, "one reply");
        // A guard that refuses (the person lost their rights) writes nothing.
        let refused = CallEntry {
            seq: 2,
            ..entry.clone()
        };
        assert_eq!(
            code(
                st.add_comment_journaled(&bot(), &t.id, reply(), &refused, &|_| {
                    Err(CoreError::invalid("forbidden", "revoked"))
                })
            ),
            "forbidden"
        );
        assert_eq!(comments(&st), 2);
        assert!(first.value()["comment_id"].is_string());
        let resolve = CallEntry {
            seq: 1,
            op: "threads.resolve".into(),
            ..entry.clone()
        };
        st.resolve_thread_journaled(&bot(), &t.id, "agent:claxbot", None, &resolve, &|_| Ok(()))
            .unwrap();
        // A created artifact and its entry commit together, or neither.
        let made = CallEntry {
            seq: 3,
            op: "artifacts.create".into(),
            ..entry.clone()
        };
        let new = || {
            let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
                "title": "Made by a run",
                "files": {"index.html": {"content": "<p>soon</p>", "encoding": "utf8"}}
            }))
            .unwrap();
            clax_core::publish::validate(req).unwrap()
        };
        let count = |st: &Store| {
            st.list_artifacts()
                .unwrap()
                .iter()
                .filter(|a| a.title == "Made by a run")
                .count()
        };
        st.set_journal_fault(Some(std::sync::Arc::new(|e: &CallEntry| e.seq == 3)));
        assert_eq!(
            code(clax_core::content::wait(st.create_artifact_journaled(
                &bot(),
                new(),
                None,
                &made,
                &|_| Ok(())
            ))),
            "journal_fault"
        );
        st.set_journal_fault(None);
        assert_eq!(count(&st), 0, "the artifact went with its entry");
        let a = clax_core::content::wait(st.create_artifact_journaled(
            &bot(),
            new(),
            None,
            &made,
            &|_| Ok(()),
        ))
        .unwrap();
        let b = clax_core::content::wait(st.create_artifact_journaled(
            &bot(),
            new(),
            None,
            &made,
            &|_| Ok(()),
        ))
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(count(&st), 1);
        assert_eq!(st.get_thread(&t.id).unwrap().unwrap().status, "resolved");
        let journal: Vec<i64> = st
            .bot_calls(&run.id, 1)
            .unwrap()
            .iter()
            .map(|c| c.seq)
            .collect();
        assert_eq!(journal, [0, 1, 3], "the refused reply journaled nothing");
    }

    #[test]
    fn minted_driver_ids_are_unique_across_threads() {
        let ids: Vec<DriverId> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8)
                .map(|_| s.spawn(|| (0..100).map(|_| DriverId::mint()).collect::<Vec<_>>()))
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        let set: std::collections::HashSet<&str> = ids.iter().map(DriverId::as_str).collect();
        assert_eq!(set.len(), ids.len());
        assert!(ids.iter().all(|d| d.as_str().starts_with("drv_")));
    }

    #[test]
    fn script_records_keep_the_code_by_hash() {
        let rec = script_record("t", "Edit", "return 1", &["files.read".into()], 3, 4, None);
        assert_eq!(
            rec.body["code_sha256"],
            format!("sha256:{}", clax_core::audit::sha256_hex(b"return 1"))
        );
        assert!(!rec.body.values().any(|v| v == "return 1"));
        let call = call_record("t", "files.write", Some("index.html"), true);
        assert_eq!(call.body["op"], "files.write");
    }
}
