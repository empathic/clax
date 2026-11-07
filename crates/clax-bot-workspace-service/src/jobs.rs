//! The jobs of [`Service`]: each call of a run is a task that goes on whoever
//! waits for it, known by its key, whose answer stays until its run ends or
//! [`Limits::answers_per_run`] newer ones push it out.
//!
//! Ending a run aborts its commands at once, since [`Workspace::finish`] waits
//! on the lock a running command holds, and the container goes next. So it
//! aborts a proposal that has not reached its push, which has left nothing. A
//! proposal past that mark is let finish, up to
//! [`Limits::proposal_settle`]: aborted there it would leave a branch with no
//! pull request, and the run could not come back to it. The mark is
//! [`Entry::pending`], which a proposal writes to the ledger before it moves
//! the branch, by push or by the forge.
//!
//! The table is memory: a restart forgets every job, which a host reads as
//! 404 ([`clax_bot_remote::wire`]), and containers and proposals stand on the
//! ledger as they do for a daemon's restart. A command's key is on disk before
//! it starts ([`crate::begun`]), so a send of its POST arriving after a restart
//! does not run it again and answers as a command lost. A proposal sent again
//! runs again: its key pushes nothing twice.
//!
//! What every answer carries is read from the ledger once and kept until a job
//! ends or a run does, the only times it changes by this service's hand. A
//! running job's first container shows in it when that job ends.

use crate::begun::Begun;
use crate::previews::PreviewLog;
use anyhow::Result;
use clax_bot::workspace::{
    CODE_SHELL_UNAVAILABLE, MAX_TIMEOUT_SECS, Offer, Proposal, Workspace, clip_output,
    shell_interrupted,
};
use clax_bot_remote::wire::{JobAnswer, JobRequest, JobState, RunRef, Snapshot, Work};
use clax_bot_sandbox::HostError;
#[cfg(doc)]
use clax_bot_workspace::ledger::Entry;
use clax_bot_workspace::ledger::Ledger;
use clax_bot_workspace::workspace::ContainerWorkspace;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::AbortHandle;

/// How long past a preview's wait, or past now while a word waits to be
/// acknowledged, a host is told to go on polling. It covers the wait's last
/// look and a host's interval between polls, so the last word is not left
/// unheard.
pub const PREVIEW_GRACE_MS: i64 = 120_000;

/// Where in a service's directory its workspace keeps the ledger
/// ([`clax_bot_workspace::config::for_home`]).
const LEDGER_DIR: &str = "workspace-runs";

/// The time now, in milliseconds since the epoch.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

struct Job {
    run: String,
    /// Its target's queue, when it has one.
    queue: Option<String>,
    /// Its place among all jobs: a larger number began later.
    number: u64,
    proposal: bool,
    /// `None` until it answers. The value outlives the task.
    answer: watch::Receiver<Option<JobState>>,
    abort: AbortHandle,
}

impl Job {
    fn running(&self) -> bool {
        matches!(state_of(&self.answer), JobState::Running)
    }
}

struct Table {
    jobs: HashMap<String, Job>,
    begun: Begun,
    next_number: u64,
    /// Runs ended since the last reap, so that a request still on its way
    /// makes nothing for them. What a reap ends goes on this list too, and
    /// anything that gets past it is the next reap's.
    ended: HashSet<String>,
}

/// A key already taken by a job of another run.
#[derive(Debug)]
pub struct KeyTaken;

/// How much a service takes on.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The most jobs running at once on one target, or on no target. A
    /// command past it is refused, and may be asked for again. A proposal is
    /// not: it is a run's last step, and may be coming back to finish.
    pub running_per_target: usize,
    /// The most ended jobs' answers held for a run. Past it the oldest goes,
    /// and its key reads as a job lost.
    pub answers_per_run: usize,
    /// How long ending a run waits for a proposal past its push, which it
    /// then aborts.
    pub proposal_settle: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            running_per_target: 16,
            answers_per_run: 64,
            proposal_settle: Duration::from_secs(300),
        }
    }
}

/// What [`Service::snapshot`] reads from the ledger.
#[derive(Clone)]
struct Offers {
    runs: BTreeMap<String, Offer>,
    queues: BTreeMap<String, Offer>,
    others: Offer,
    private_runs: BTreeSet<String>,
    private_queues: BTreeSet<String>,
    /// When the last preview's wait ends, grace and all.
    waited_ms: Option<i64>,
}

/// A [`ContainerWorkspace`] and what the wire adds to it.
pub struct Service {
    workspace: ContainerWorkspace,
    /// The workspace's own, read here for how far a proposal has gone.
    ledger: Ledger,
    table: Mutex<Table>,
    previews: Arc<PreviewLog>,
    clock: Clock,
    limits: Limits,
    offers: Mutex<Option<Offers>>,
    /// Counts what changes the ledger, so that a read begun before a change
    /// is not kept after it.
    changes: AtomicU64,
    /// The greatest [`Snapshot::seq`] given.
    seq: AtomicU64,
}

/// A run known by its ID alone, for what reads no more of it.
fn bare_run(id: &str) -> RunRef {
    RunRef {
        id: id.to_string(),
        thread_ids: Vec::new(),
    }
}

fn failed(e: HostError) -> JobState {
    JobState::Failed {
        code: e.code,
        message: e.message,
        retryable: e.retryable,
    }
}

/// Where a job stands. One whose task died with no answer was aborted or
/// panicked: what became of it cannot be said.
fn state_of(answer: &watch::Receiver<Option<JobState>>) -> JobState {
    match &*answer.borrow() {
        Some(state) => state.clone(),
        None if answer.has_changed().is_err() => failed(shell_interrupted()),
        None => JobState::Running,
    }
}

/// Until the task behind `answer` is gone, and any lock it held with it.
async fn gone(answer: &mut watch::Receiver<Option<JobState>>) {
    while answer.changed().await.is_ok() {}
}

impl Service {
    /// Over `workspace`, keeping its files in `dir`. It takes up the preview
    /// waits the workspace's ledger holds, so it wants a runtime.
    ///
    /// # Errors
    /// When the told file or the begun keys cannot be read.
    pub fn open(workspace: ContainerWorkspace, dir: &Path) -> Result<Arc<Service>> {
        Service::open_with(workspace, dir, Arc::new(system_clock), Limits::default())
    }

    /// [`Service::open`], reading the time from `clock`, within `limits`.
    ///
    /// # Errors
    /// When the told file or the begun keys cannot be read.
    pub fn open_with(
        workspace: ContainerWorkspace,
        dir: &Path,
        clock: Clock,
        limits: Limits,
    ) -> Result<Arc<Service>> {
        let previews = Arc::new(PreviewLog::open(dir)?);
        let table = Table {
            jobs: HashMap::new(),
            begun: Begun::open(dir)?,
            next_number: 0,
            ended: HashSet::new(),
        };
        workspace.previews(previews.clone());
        Ok(Arc::new(Service {
            workspace,
            ledger: Ledger::at(dir.join(LEDGER_DIR)),
            table: Mutex::new(table),
            previews,
            clock,
            limits,
            offers: Mutex::default(),
            changes: AtomicU64::new(0),
            seq: AtomicU64::new(0),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_offers(&self) -> MutexGuard<'_, Option<Offers>> {
        self.offers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The told file.
    #[must_use]
    pub fn previews(&self) -> &Arc<PreviewLog> {
        &self.previews
    }

    /// Reads what the workspace offers, each answer its own
    /// [`Workspace::offer`]'s. The blank ID is no run's: the wire refuses it.
    fn read_offers(&self) -> Offers {
        let ws = &self.workspace;
        let nobody = bare_run("").to_run();
        let (mut runs, mut private_runs) = (BTreeMap::new(), BTreeSet::new());
        for (id, queue) in ws.runs_keeping_offers() {
            let run = bare_run(&id).to_run();
            if ws.target_private(&run, queue.as_deref()) {
                private_runs.insert(id.clone());
            }
            runs.insert(id, ws.offer(&run, queue.as_deref()));
        }
        let (mut queues, mut private_queues) = (BTreeMap::new(), BTreeSet::new());
        for q in ws.operator_queues() {
            if ws.target_private(&nobody, Some(&q)) {
                private_queues.insert(q.clone());
            }
            let offer = ws.offer(&nobody, Some(&q));
            queues.insert(q, offer);
        }
        let waited = ws.previews_until().map(|s| {
            i64::try_from(s)
                .unwrap_or(i64::MAX / 2)
                .saturating_mul(1000)
        });
        Offers {
            runs,
            queues,
            others: ws.offer(&nobody, None),
            private_runs,
            private_queues,
            waited_ms: waited.map(|ms| ms.saturating_add(PREVIEW_GRACE_MS)),
        }
    }

    /// What was read last, if nothing has changed since.
    fn offers(&self) -> Offers {
        if let Some(kept) = &*self.lock_offers() {
            return kept.clone();
        }
        let before = self.changes.load(Ordering::SeqCst);
        let read = self.read_offers();
        let mut kept = self.lock_offers();
        if self.changes.load(Ordering::SeqCst) == before {
            *kept = Some(read.clone());
        }
        read
    }

    /// The ledger may say something new.
    fn changed(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        *self.lock_offers() = None;
    }

    /// Everything the workspace offers. Its number is the time, so it holds
    /// across a restart, and never goes back within one. It is taken before
    /// the offers are, so what was read earlier never has the greater number.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let now = u64::try_from((self.clock)()).unwrap_or(0);
        let seq = self.seq.fetch_max(now, Ordering::SeqCst).max(now);
        let offers = self.offers();
        let unheard = (!self.previews.is_empty()).then(|| (self.clock)() + PREVIEW_GRACE_MS);
        Snapshot {
            seq,
            runs: offers.runs,
            queues: offers.queues,
            others: offers.others,
            private_runs: offers.private_runs,
            private_queues: offers.private_queues,
            previews_until_ms: offers.waited_ms.max(unheard),
        }
    }

    fn answer(&self, state: JobState) -> JobAnswer {
        JobAnswer {
            state,
            snapshot: self.snapshot(),
        }
    }

    /// What `request` calls for, by the workspace.
    async fn work(&self, request: &JobRequest) -> Result<Value, HostError> {
        let run = request.run.to_run();
        match &request.work {
            Work::Exec {
                queue,
                command,
                cwd,
                timeout_secs,
            } => {
                let timeout = (*timeout_secs).clamp(1, MAX_TIMEOUT_SECS);
                let (queue, cwd) = (queue.as_deref(), cwd.as_deref());
                let mut done = self
                    .workspace
                    .exec(&run, queue, command, cwd, timeout)
                    .await?;
                if let Some(text) = done["output"].as_str() {
                    done["output"] = Value::String(clip_output(text));
                }
                Ok(done)
            }
            Work::Propose { title, summary } => {
                let proposal = Proposal {
                    title: title.clone(),
                    summary: summary.clone(),
                    key: request.key.clone(),
                };
                self.workspace.propose(&run, &proposal).await
            }
        }
    }

    /// The queue of `request`'s target. A proposal does not say: its run's
    /// container does.
    fn queue_of(&self, table: &Table, request: &JobRequest) -> Option<String> {
        if let Work::Exec { queue, .. } = &request.work {
            return queue.clone();
        }
        let id = &request.run.id;
        let of_run = table.jobs.values().find(|j| &j.run == id);
        match of_run {
            Some(job) => job.queue.clone(),
            None => {
                let runs = self.workspace.runs_keeping_offers();
                runs.into_iter().find(|(r, _)| r == id)?.1
            }
        }
    }

    /// Makes room for one more of run `id`'s answers.
    fn make_room(&self, table: &mut Table, id: &str) {
        let of_run = table
            .jobs
            .iter()
            .filter(|(_, j)| j.run == id && !j.running());
        let mut ended: Vec<(u64, String)> =
            of_run.map(|(key, j)| (j.number, key.clone())).collect();
        ended.sort_unstable();
        let over = (ended.len() + 1).saturating_sub(self.limits.answers_per_run);
        for (_, key) in ended.into_iter().take(over) {
            table.jobs.remove(&key);
        }
    }

    /// Starts `request`'s job in `table`, past every check.
    fn begin(self: &Arc<Service>, table: &mut Table, queue: Option<String>, request: JobRequest) {
        let (key, run) = (request.key.clone(), request.run.id.clone());
        let proposal = matches!(request.work, Work::Propose { .. });
        // Never the command: it is the model's, and may hold anything.
        let kind = if proposal { "propose" } else { "exec" };
        tracing::info!(
            target: "audit",
            run,
            key,
            queue = queue.as_deref().unwrap_or(""),
            kind,
            "a job began"
        );
        self.make_room(table, &run);
        let (tell, answer) = watch::channel(None);
        let service = self.clone();
        let task = tokio::spawn(async move {
            let state = match service.work(&request).await {
                Ok(value) => JobState::Done { value },
                Err(e) => failed(e),
            };
            service.changed();
            tell.send_replace(Some(state));
        });
        let number = table.next_number;
        table.next_number += 1;
        let job = Job {
            run,
            queue,
            number,
            proposal,
            answer,
            abort: task.abort_handle(),
        };
        table.jobs.insert(key, job);
    }

    /// What is to become of `request`, and starts its job if that is to run.
    fn take(self: &Arc<Service>, request: JobRequest) -> Result<JobState, KeyTaken> {
        let mut table = self.lock();
        let table = &mut *table;
        let id = &request.run.id;
        if table.ended.contains(id) {
            let ended = HostError::new(CODE_SHELL_UNAVAILABLE, "this run has ended");
            return Ok(failed(ended));
        }
        if let Some(job) = table.jobs.get(&request.key) {
            if &job.run != id {
                return Err(KeyTaken);
            }
            let state = state_of(&job.answer);
            let again = matches!(
                state,
                JobState::Failed {
                    retryable: true,
                    ..
                }
            );
            if !(job.proposal && again) {
                return Ok(state);
            }
            // Told it may try again, it does: nothing was published.
            table.jobs.remove(&request.key);
        }
        let exec = matches!(request.work, Work::Exec { .. });
        match table.begun.run_of(&request.key) {
            Some(run) if run != id => return Err(KeyTaken),
            // Begun, and its answer gone: it may have run.
            Some(_) if exec => return Ok(failed(shell_interrupted())),
            _ => {}
        }
        let queue = self.queue_of(table, &request);
        let running = table
            .jobs
            .values()
            .filter(|j| j.queue == queue && j.running());
        if exec && running.count() >= self.limits.running_per_target {
            let busy = HostError::new(
                CODE_SHELL_UNAVAILABLE,
                "too many commands are running on this repository: nothing was run. Try again \
                 shortly.",
            );
            return Ok(failed(busy.retryable()));
        }
        if exec && let Err(e) = table.begun.note(&request.key, id) {
            tracing::warn!(error = %e, run = id, "a job's key could not be kept");
            let unkept = HostError::new(
                CODE_SHELL_UNAVAILABLE,
                "the workspace service could not keep this command's place: nothing was run. Try \
                 again shortly.",
            );
            return Ok(failed(unkept.retryable()));
        }
        self.begin(table, queue, request);
        Ok(JobState::Running)
    }

    /// Starts `request`'s job, or joins the one under its key, running or
    /// ended, and answers at once.
    ///
    /// # Errors
    /// When the key is another run's.
    pub fn start(self: &Arc<Service>, request: JobRequest) -> Result<JobAnswer, KeyTaken> {
        let state = self.take(request)?;
        Ok(self.answer(state))
    }

    /// Job `key`, once it ends or `wait` is up; `None` when there is no such
    /// job.
    pub async fn poll(&self, key: &str, wait: Duration) -> Option<JobAnswer> {
        let mut answer = self.lock().jobs.get(key)?.answer.clone();
        let ended = async { answer.wait_for(Option::is_some).await.map(|_| ()) };
        // Either way, what it says now is the answer.
        let _ = tokio::time::timeout(wait, ended).await;
        let state = state_of(&answer);
        Some(self.answer(state))
    }

    /// Whether run `id`'s proposal `key` has reached its push. A ledger that
    /// cannot be read says it may have.
    fn past_its_push(&self, id: &str, key: &str) -> bool {
        self.ledger.read(id).map_or(true, |entry| {
            let pending = entry.pending.is_some_and(|p| p.key == key);
            pending || entry.proposals.contains_key(key)
        })
    }

    /// Ends run `id`: its commands and its proposals short of their push at
    /// once, those past it once they settle, then its container. Its answers
    /// go with it. Ended again it does no harm.
    pub async fn end(&self, id: &str) -> Snapshot {
        let (mut aborted, mut proposals) = (Vec::new(), Vec::new());
        {
            let mut table = self.lock();
            table.ended.insert(id.to_string());
            for (key, job) in table.jobs.iter().filter(|(_, j)| j.run == id) {
                if job.proposal && job.running() && self.past_its_push(id, key) {
                    proposals.push((job.answer.clone(), job.abort.clone()));
                } else {
                    job.abort.abort();
                    aborted.push(job.answer.clone());
                }
            }
        }
        for answer in &mut aborted {
            gone(answer).await;
        }
        for (answer, abort) in &mut proposals {
            let settle = self.limits.proposal_settle;
            if tokio::time::timeout(settle, gone(answer)).await.is_err() {
                tracing::warn!(run = id, "a proposal did not settle, and was aborted");
                abort.abort();
                gone(answer).await;
            }
        }
        self.workspace.finish(&bare_run(id).to_run()).await;
        {
            let mut table = self.lock();
            table.jobs.retain(|_, j| j.run != id);
            table.begun.forget(id);
        }
        self.changed();
        self.snapshot()
    }

    /// Ends every run not in `active`: those this process holds, whose
    /// containers [`Workspace::reap`] leaves alone, and those whose keys a
    /// restart left, and then whatever else the runtime has under the
    /// workspace's labels.
    pub async fn reap(&self, active: &[String]) -> Snapshot {
        let over: HashSet<String> = {
            let mut table = self.lock();
            table.ended.clear();
            let runs = table.jobs.values().map(|j| &j.run);
            let runs = runs.chain(table.begun.runs());
            runs.filter(|r| !active.contains(r)).cloned().collect()
        };
        for id in over {
            self.end(&id).await;
        }
        self.workspace.reap(active).await;
        self.changed();
        self.snapshot()
    }
}
