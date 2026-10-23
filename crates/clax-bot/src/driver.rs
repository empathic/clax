//! Drives runs: a runtime-agnostic scheduler over [`crate::run::tick`].
//!
//! A [`Driver`] keeps a queue of runs to tick. [`Driver::wake`] queues one
//! (a new run, a cancel, a run whose wait is over); [`Driver::run_until_idle`]
//! ticks queued runs, at most `concurrency` at once and at most
//! `per_person_concurrency` of one person's, starting a run woken while
//! others tick at once, until none is runnable or its wall-time budget is
//! spent (the Worker's alarm, which gives it at least
//! [`crate::RunSettings::min_turn_budget`]); [`Driver::spawn_forever`]
//! does that in a loop on a background task, sleeping until the next wake
//! (the native daemon). Everything it awaits is plain futures and
//! [`clax_rt::time`], so it runs on tokio and on the Worker's event loop
//! alike.
//!
//! A tick claims its run and renews the claim while it works; the driver
//! releases a run's claim when it stops ticking it (waiting or ended) and
//! on [`Driver::release_all`], so another driver may take it at once.
//!
//! Waits are kept by the deps' clock (injected in tests): a run whose
//! sandbox was unavailable ([`Wait::Stalled`]) waits with exponential
//! backoff (1 s doubling to 60 s), uncharged; a run waiting for the model
//! waits what the tick said; a run whose tick had too little budget left
//! ([`Wait::Budget`]) is due at once, in the next call. Each wait's end is
//! recorded on the run (`due_at`), and a driver that starts later
//! ([`Driver::resume`]) waits it out rather than ticking the run early.

use crate::events::EventHub;
use crate::run::{Deps, Next, RunSettings, TickOpts, TickOutcome, Wait, tick};
use chrono::{DateTime, Utc};
use clax_bot_sandbox::CancelToken;
use clax_store::bot_runs::DriverId;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The first and the longest wait of a stalled run.
const STALL_FIRST: Duration = Duration::from_secs(1);
const STALL_MAX: Duration = Duration::from_secs(60);
/// How long a run another driver holds waits before it is looked at again.
const NOT_OURS_WAIT: Duration = Duration::from_secs(30);

struct Active {
    person: String,
    cancel: CancelToken,
}

#[derive(Default)]
struct State {
    queue: VecDeque<String>,
    delayed: Vec<(DateTime<Utc>, String)>,
    /// Runs to tick in the next call, not this one.
    deferred: Vec<String>,
    active: HashMap<String, Active>,
    stalls: HashMap<String, u32>,
    /// Runs cancelled while not active, cancelled when they start.
    cancelled: Vec<String>,
    /// Runs whose wait is recorded in the store (`due_at`), to clear when
    /// they next run.
    recorded: std::collections::HashSet<String>,
}

/// What [`Driver::run_until_idle`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdleReport {
    /// Every tick it ran, by run.
    pub ticks: Vec<(String, TickOutcome)>,
    /// When to call it again for a run that waits; `Some(ZERO)` when runs
    /// are still queued (the budget ran out, or a tick needs a fresh one);
    /// `None` when nothing waits.
    pub next_wake: Option<Duration>,
}

/// The scheduler of one driver (one process natively, one Durable Object
/// incarnation on the Worker).
pub struct Driver {
    id: DriverId,
    deps: Arc<Deps>,
    events: Arc<EventHub>,
    state: Mutex<State>,
    wakes: tokio::sync::Notify,
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn to_chrono(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX)
}

impl Driver {
    #[must_use]
    pub fn new(id: DriverId, deps: Arc<Deps>, events: Arc<EventHub>) -> Driver {
        Driver {
            id,
            deps,
            events,
            state: Mutex::new(State::default()),
            wakes: tokio::sync::Notify::new(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &DriverId {
        &self.id
    }

    /// The runs' live events.
    #[must_use]
    pub fn events(&self) -> &Arc<EventHub> {
        &self.events
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> DateTime<Utc> {
        self.deps.clock.now()
    }

    /// Queues run `run_id` to be ticked (a waiting run is ticked at once,
    /// and a running [`Driver::run_until_idle`] starts it).
    pub fn wake(&self, run_id: &str) {
        {
            let mut st = self.lock();
            st.delayed.retain(|(_, r)| r != run_id);
            st.deferred.retain(|r| r != run_id);
            if !st.queue.iter().any(|r| r == run_id) && !st.active.contains_key(run_id) {
                st.queue.push_back(run_id.to_string());
            }
        }
        self.wakes.notify_one();
    }

    /// Stops run `run_id`'s step at once, if this driver is working on it
    /// (after [`clax_store::Store::cancel_run`] made it `stopping`), and
    /// queues it so its `stopped` step is written.
    pub fn cancel(&self, run_id: &str) {
        {
            let mut st = self.lock();
            match st.active.get(run_id) {
                Some(a) => a.cancel.cancel(),
                None => st.cancelled.push(run_id.to_string()),
            }
        }
        self.wake(run_id);
    }

    /// Queues every run that has not ended (on start), each at once or,
    /// when the driver that let it wait recorded when ([`clax_store::bot_runs::Run::due_at`],
    /// written as it settles each tick), then. With `take_over`,
    /// first takes each one's claim from whoever held it (a native daemon
    /// is its home's only driver, so claims from before a restart are its
    /// own dead ones).
    ///
    /// # Errors
    /// The store's.
    pub fn resume(&self, take_over: bool) -> clax_core::Result<usize> {
        let runs = self.deps.store.live_runs()?;
        let now = self.now();
        for r in &runs {
            if take_over {
                self.deps.store.claim_run(&r.id, &self.id, 0)?;
            }
            // A wait an earlier driver recorded is honoured (a cancelled
            // run is woken by its cancel all the same).
            let due = r
                .due_at
                .as_deref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc))
                .filter(|at| *at > now && r.status != clax_store::bot_runs::RunStatus::Stopping);
            match due {
                Some(at) => {
                    let mut st = self.lock();
                    st.delayed.retain(|(_, x)| *x != r.id);
                    st.delayed.push((at, r.id.clone()));
                    st.recorded.insert(r.id.clone());
                }
                None => {
                    if r.due_at.is_some() {
                        // Cleared when it next settles.
                        self.lock().recorded.insert(r.id.clone());
                    }
                    self.wake(&r.id);
                }
            }
        }
        Ok(runs.len())
    }

    /// What a driver does when it starts, on the daemon and the Worker
    /// alike: removes the artifacts with no version older than a claim's
    /// lease (a crash between `artifacts.create`'s artifact row and its
    /// first version leaves one, which would count toward its person's
    /// quota), starts the runs that were waiting on a run's end
    /// ([`crate::host::BotHost::start_waiting`]: follow-ups a crash left queued), then
    /// queues every run that has not ended ([`Driver::resume`]). A failed
    /// sweep or start is logged and does not stop the resume. Returns the
    /// runs queued.
    ///
    /// # Errors
    /// The store's.
    pub async fn start(&self, take_over: bool) -> clax_core::Result<usize> {
        let lease = self
            .deps
            .settings
            .settings()
            .map_or(RunSettings::default().lease_secs, |s| s.lease_secs);
        // The sweep is housekeeping: its failure is logged, and the runs
        // resume whatever it did.
        match self
            .deps
            .store
            .sweep_zero_version_artifacts(chrono::Duration::seconds(lease))
            .await
        {
            Ok(0) => {}
            Ok(swept) => tracing::info!(swept, "removed artifacts a crash left without a version"),
            Err(e) => tracing::warn!(error = %e, "the sweep of artifacts without a version failed"),
        }
        match self.deps.host.start_waiting().await {
            Ok(0) => {}
            Ok(started) => {
                tracing::info!(started, "started the follow-ups a run's end left waiting")
            }
            Err(e) => tracing::warn!(error = %e, "the waiting follow-ups did not start"),
        }
        self.resume(take_over)
    }

    /// Lets go of every claim this driver holds that it is not ticking.
    pub fn release_all(&self) {
        let ids: Vec<String> = {
            let st = self.lock();
            st.queue
                .iter()
                .chain(st.delayed.iter().map(|(_, r)| r))
                .chain(st.deferred.iter())
                .cloned()
                .collect()
        };
        for id in ids {
            let _ = self.deps.store.release_run(&id, &self.id);
        }
    }

    /// The next queued run allowed to start now, by the caps.
    fn take_next(&self, concurrency: usize, per_person: usize) -> Option<(String, CancelToken)> {
        let now = self.now();
        let mut st = self.lock();
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut st.delayed)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        st.delayed = later;
        for (_, r) in due {
            if !st.queue.contains(&r) {
                st.queue.push_back(r);
            }
        }
        if st.active.len() >= concurrency {
            return None;
        }
        let mut skipped = Vec::new();
        let mut picked = None;
        while let Some(id) = st.queue.pop_front() {
            if st.active.contains_key(&id) {
                continue;
            }
            let person = match self.deps.store.run(&id) {
                Ok(Some(r)) if !r.status.is_terminal() => r.person,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("clax-bot: reading run {id}: {e}");
                    skipped.push(id);
                    continue;
                }
            };
            let busy = st.active.values().filter(|a| a.person == person).count();
            if busy >= per_person {
                skipped.push(id);
                continue;
            }
            let cancel = CancelToken::new();
            if let Some(i) = st.cancelled.iter().position(|c| *c == id) {
                st.cancelled.remove(i);
                cancel.cancel();
            }
            st.active.insert(
                id.clone(),
                Active {
                    person,
                    cancel: cancel.clone(),
                },
            );
            picked = Some((id, cancel));
            break;
        }
        for id in skipped.into_iter().rev() {
            st.queue.push_front(id);
        }
        picked
    }

    /// Records what a tick of `id` did and decides when it runs next.
    fn settle(&self, id: &str, out: &clax_core::Result<TickOutcome>) {
        let now = self.now();
        let mut st = self.lock();
        st.active.remove(id);
        let wait = match out {
            Ok(o) => match o.next {
                Next::Runnable => {
                    st.stalls.remove(id);
                    st.queue.push_back(id.to_string());
                    None
                }
                Next::Done(_) => {
                    st.stalls.remove(id);
                    None
                }
                Next::Waiting(Wait::Stalled) => {
                    let n = st.stalls.entry(id.to_string()).or_insert(0);
                    let d = STALL_FIRST
                        .saturating_mul(1u32.checked_shl(*n).unwrap_or(u32::MAX))
                        .min(STALL_MAX);
                    *n += 1;
                    Some(d)
                }
                Next::Waiting(Wait::Retry(d)) => Some(d),
                Next::Waiting(Wait::NotOurs) => Some(NOT_OURS_WAIT),
                Next::Waiting(Wait::Budget) => {
                    st.deferred.push(id.to_string());
                    None
                }
                // A test's crash: the run waits for another driver.
                Next::Waiting(Wait::Crashed) => None,
            },
            Err(e) => {
                tracing::warn!("clax-bot: ticking run {id}: {e}");
                Some(NOT_OURS_WAIT)
            }
        };
        // The wait is recorded, so a driver that starts after this one
        // stops (a restart, an eviction) waits it out too.
        let record = match wait {
            Some(d) => {
                let at = now + to_chrono(d);
                st.delayed.push((at, id.to_string()));
                st.recorded.insert(id.to_string());
                Some(Some(at))
            }
            None => st.recorded.remove(id).then_some(None),
        };
        drop(st);
        if let Some(at) = record
            && let Err(e) = self.deps.store.set_run_due(id, at)
        {
            tracing::warn!("clax-bot: recording run {id}'s wait: {e}");
        }
        let keep = matches!(out, Ok(o) if matches!(o.next, Next::Runnable | Next::Waiting(Wait::Crashed | Wait::Budget)));
        if !keep {
            let _ = self.deps.store.release_run(id, &self.id);
        }
    }

    /// The time until the first waiting run is due, by the deps' clock.
    fn until_due(&self) -> Option<Duration> {
        let now = self.now();
        self.lock()
            .delayed
            .iter()
            .map(|(at, _)| (*at - now).to_std().unwrap_or_default())
            .min()
    }

    /// Ticks queued runs until none can start or `budget` is spent; a tick
    /// already started finishes (its model call is bounded by what is left
    /// of the budget). A run woken (or falling due) while others tick
    /// starts as soon as the caps allow.
    pub async fn run_until_idle(&self, budget: Duration) -> IdleReport {
        let start = clax_rt::time::Instant::now();
        let mut report = IdleReport::default();
        let mut running = FuturesUnordered::new();
        loop {
            let settings = match self.deps.settings.settings() {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("clax-bot: reading settings: {e}");
                    break;
                }
            };
            let concurrency = settings.concurrency.max(1) as usize;
            let per_person = settings.per_person_concurrency.max(1) as usize;
            let left = budget.saturating_sub(start.elapsed());
            if !left.is_zero() {
                while let Some((id, cancel)) = self.take_next(concurrency, per_person) {
                    let deps = &*self.deps;
                    let events = &*self.events;
                    let driver = &self.id;
                    running.push(async move {
                        let out = tick(
                            deps,
                            &id,
                            driver,
                            TickOpts {
                                budget: Some(left),
                                cancel,
                                events,
                            },
                        )
                        .await;
                        (id, out)
                    });
                }
            }
            if running.is_empty() {
                break;
            }
            // A tick ends, a run is woken, or a waiting one falls due.
            let woken = self.wakes.notified();
            let idle = Duration::from_secs(3600);
            let due = clax_rt::time::sleep(if left.is_zero() {
                idle
            } else {
                self.until_due().unwrap_or(idle)
            });
            tokio::select! {
                Some((id, out)) = running.next() => {
                    self.settle(&id, &out);
                    if let Ok(o) = out {
                        report.ticks.push((id, o));
                    }
                }
                () = woken => {}
                () = due => {}
            }
        }
        let mut st = self.lock();
        let deferred = std::mem::take(&mut st.deferred);
        st.queue.extend(deferred);
        report.next_wake = if st.queue.is_empty() {
            drop(st);
            self.until_due()
        } else {
            Some(Duration::ZERO)
        };
        report
    }

    /// Drives runs for as long as the process lives: ticks until idle,
    /// then sleeps until a wake or the next waiting run is due.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_forever(self: Arc<Self>) -> clax_rt::JoinHandle<()> {
        clax_rt::spawn(async move {
            loop {
                let report = self.run_until_idle(Duration::from_secs(3600)).await;
                let notified = self.wakes.notified();
                match report.next_wake {
                    Some(d) if d.is_zero() => continue,
                    Some(d) => {
                        let _ = clax_rt::time::timeout(d, notified).await;
                    }
                    None => notified.await,
                }
            }
        })
    }
}
