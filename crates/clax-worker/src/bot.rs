//! ClaxBot on the Worker (spec
//! `docs/superpowers/specs/2026-10-21-claxbot-design.md` §4.2, §8.1): the
//! instance object drives runs from its alarm.
//!
//! # What runs a run
//!
//! [`WorkerBot`] is made with the server, once per object incarnation (and
//! again by the test route that rebuilds the server, as a wake after
//! eviction does). It holds a [`Driver`] whose [`DriverId`] is minted then,
//! over [`Deps`]: the instance's store and clock, a [`MessagesClient`]
//! (`fetch` on wasm32), the Worker Loader sandbox over the `LOADER`
//! binding with the `clax` API's operations ([`sandbox`]), the server's
//! own host (`clax_server::bot::ServerHost`, as on the daemon), the runs'
//! live events (the [`EventHub`] the routes stream), the settings the admin
//! pages keep in the database ([`StoredSettings`]: the Worker has no
//! config file, and it charges the Durable Object request and Toolpath
//! storage lines, `infra_lines`), and the keys: the key-encryption key
//! from the `CLAX_KEK` secret ([`kek_of`]) and no environment key.
//!
//! At the incarnation's first event the driver starts ([`Driver::start`]
//! with `take_over`: the artifacts a crash left without a version are
//! swept, then every run that has not ended is queued, its claim taken
//! back, honouring a wait an earlier driver recorded). Taking claims back
//! is safe here: the runs are in this object's own storage, which only
//! one incarnation of the object writes at a time, so a claim another
//! driver ID holds is an earlier incarnation's, which is gone; and were
//! the old one still finishing a step, its next lease renewal or step
//! write finds the claim taken and it stops (`Wait::NotOurs`), committing
//! nothing. Without the take-over, a run cut by an eviction would wait out
//! the dead driver's lease.
//!
//! # When it runs
//!
//! The routes' run seam is the daemon's (`clax_server::bot::DriverRuns`),
//! which wakes or cancels the run in the driver at once, wrapped in
//! [`Kicks`], which notes that one came; after the request the object
//! ([`WorkerBot::take_wakes`]) wants the alarm now, unless one is already
//! driving (that one starts the run itself). The alarm calls
//! [`WorkerBot::drive`]: [`Driver::run_until_idle`] for [`alarm_budget`],
//! as many steps of as many runs as fit, after which the driver's
//! `next_wake` is when the alarm is wanted again (merged with the
//! instance's other timed work, `crate::instance`; [`Schedule`] holds the
//! rules). No timer outlives the alarm: between ticks the object may
//! hibernate or be evicted, and the alarm wakes it.
//!
//! # The alarm's budget
//!
//! Cloudflare bounds one alarm invocation at 15 minutes of wall time
//! ([`ALARM_WALL_LIMIT`]; developers.cloudflare.com/workers/platform/limits,
//! "Duration": "Cron Triggers, Durable Object Alarms and Queue Consumers:
//! 15 minutes") and 30 seconds of CPU time by default (the same page, "CPU
//! time"; the Worker sets no `limits.cpu_ms`). A run's ticks wait on the
//! model's stream and on the sandbox's own isolates, which spend none of
//! the object's CPU, so the wall limit is the one the budget is sized by:
//! [`alarm_budget`] is the wall limit less [`ALARM_MARGIN`] (5 minutes, for
//! the step a tick started near the budget's end to finish and commit,
//! and the rest of the alarm's work), and never less than
//! `RunSettings::min_turn_budget` (60 s by default), the least a turn is
//! started with. A tick with less than that left waits for the next
//! alarm, which is due at once. While it drives, the alarm is kept set
//! [`RESUME_AFTER`] ahead (renewed as it passes, at most every
//! [`RESUME_AFTER`]), so an invocation the platform ends early (an
//! eviction, the CPU limit) is followed within that by another, whose
//! new incarnation resumes the run's uncommitted step; a request to the
//! object (the run's page polling, its event stream reconnecting)
//! resumes it at once.
//!
//! # Its workspace
//!
//! With a workspace service set up ([`crate::workspace`]) the driver and the
//! host share a [`WorkerWorkspace`]. A call on it goes on at the service, so
//! a tick gives it up when its time is out and the next joins it: a command
//! may outlive any number of alarms. The host follows reports' changes when
//! asked ([`WorkerBot::follow_reports`]), after each request and around the
//! alarm's drive. The workspace's timed work ([`WorkerBot::tend`]) retries
//! ends the service did not answer and polls for previews, for as long as it
//! says one may come, whether or not its run has ended, and on after a poll
//! that failed. While it keeps failing it is held back, up to a cap, so a
//! service that will not answer costs an alarm now and then ([`tend_due`]).
//!
//! Every connection a Worker has waiting for headers counts against the
//! platform's six, and each run on a workspace holds one in its polls, so
//! such a Worker runs at most [`WORKSPACE_CONCURRENCY`] at once.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clax_bot::{BotHost, Deps, Driver, EventHub, Keys, RunSettings, SettingsSource};
use clax_sample::messages::MessagesClient;
use clax_store::Store;
use clax_store::bot_runs::DriverId;
use clax_store::secrets::Kek;

use clax_bot_sandbox::Sandbox;

use crate::workspace::{
    REQUEST_TEND_DEADLINE, TEND_DEADLINE, TEND_EVERY, TEND_FOR_A_REQUEST_AFTER, WorkerWorkspace,
};

/// Cloudflare's wall-time limit on one Durable Object alarm invocation.
pub const ALARM_WALL_LIMIT: Duration = Duration::from_secs(15 * 60);
/// What [`alarm_budget`] leaves of [`ALARM_WALL_LIMIT`] for the step a tick
/// started near the budget's end and the rest of the alarm's work.
pub const ALARM_MARGIN: Duration = Duration::from_secs(5 * 60);
/// How far ahead the alarm is kept while one drives (see the module docs).
pub const RESUME_AFTER: Duration = Duration::from_secs(60);
/// The most runs at once on a Worker with a workspace: one connection each,
/// and one left of the platform's six for the preview poll.
pub const WORKSPACE_CONCURRENCY: u32 = 5;

/// The wall time one alarm gives the driver (see the module docs).
#[must_use]
pub fn alarm_budget(min_turn_budget: Duration) -> Duration {
    ALARM_WALL_LIMIT
        .saturating_sub(ALARM_MARGIN)
        .max(min_turn_budget)
}

/// When the workspace's timed work is next wanted (ms): `every` after the last
/// time it was done, or when what held it back ends, if that is later.
#[must_use]
pub fn tend_due(last_tend: f64, every: Duration, held_until_ms: Option<i64>) -> f64 {
    let due = last_tend + every.as_millis() as f64;
    held_until_ms.map_or(due, |held| due.max(held as f64))
}

/// The key-encryption key `secret` (the `CLAX_KEK` secret: 64 hex digits,
/// 32 bytes) is: `Ok(None)` when it is unset, `Err` with why when it is
/// not a key. The error never carries the secret.
///
/// # Errors
/// As above.
pub fn kek_of(secret: &str) -> Result<Option<Kek>, &'static str> {
    let s = secret.trim();
    if s.is_empty() {
        return Ok(None);
    }
    const NOT_A_KEY: &str = "CLAX_KEK must be 64 hex digits (32 random bytes)";
    if s.len() != 64 {
        return Err(NOT_A_KEY);
    }
    let mut bytes = [0u8; 32];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16).ok_or(NOT_A_KEY)?;
        let lo = (pair[1] as char).to_digit(16).ok_or(NOT_A_KEY)?;
        bytes[i] = (hi * 16 + lo) as u8;
    }
    Ok(Some(Kek::from_bytes(bytes)))
}

/// ClaxBot's sandbox on this Worker: [`crate::sandbox::LoaderSandbox`]
/// over the `LOADER` binding, whose `clax` has the API's operations.
///
/// # Errors
/// Why the sandbox refused the operations.
#[cfg(target_arch = "wasm32")]
pub fn sandbox(env: &worker::Env) -> Result<Arc<dyn Sandbox>, String> {
    let s = crate::sandbox::LoaderSandbox::from_env(env, clax_bot::api::op_names())?;
    Ok(Arc::new(s))
}

/// The Worker Loader sandbox runs on wasm32 only.
///
/// # Errors
/// Always, natively.
#[cfg(not(target_arch = "wasm32"))]
pub fn sandbox(_env: &worker::Env) -> Result<Arc<dyn Sandbox>, String> {
    Err("the Worker Loader sandbox is a Worker's (wasm32)".into())
}

/// Where ClaxBot's `clax.web` calls go on this Worker
/// (`crate::web_source`): `test`, the scripted source of the test mode,
/// when given; else the `AI` binding, when the Worker has one, its fetches
/// naming the instance at `public_origin` (`PUBLIC_ORIGIN`, when set; never
/// a fallback) in their `User-Agent`; else none, and the host's web use is
/// off, not configured. The source answers `public_origin`'s host and
/// `site_domain` (with every host under it) as the instance's own, which
/// no fetch reaches ([`own_hosts`]). The binding's source records in
/// `check` what it finds of whether it can run where the Worker runs
/// (`clax_bot::WebSource::start_check`), and hands its check to the
/// Durable Object's `state` to keep it alive.
///
/// # Errors
/// Why `public_origin` or `site_domain`, when set, names no host: the
/// instance could not tell which hosts are its own, so it has no source
/// ([`web_or_reason`]).
pub fn web_source(
    env: &worker::Env,
    test: Option<Arc<crate::web_source::FixtureWeb>>,
    public_origin: Option<&str>,
    site_domain: Option<&str>,
    check: Arc<clax_bot::web::HostCheck>,
    state: &worker::State,
) -> Result<Option<Arc<dyn clax_bot::WebSource>>, String> {
    let source: Arc<dyn clax_bot::WebSource> = match test {
        Some(t) => t,
        None => match binding_source(env, public_origin, check, state) {
            Some(s) => s,
            None => return Ok(None),
        },
    };
    own_hosts(source, public_origin, site_domain).map(Some)
}

/// `source`, answering `public_origin`'s host and `site_domain` as the
/// instance's own hosts (`clax_bot::web::WithOwnHosts`); `source` itself
/// when the Worker has neither.
///
/// # Errors
/// Why a set `public_origin` or `site_domain` names no host
/// (`clax_bot::web::OwnHosts::new`).
pub fn own_hosts(
    source: Arc<dyn clax_bot::WebSource>,
    public_origin: Option<&str>,
    site_domain: Option<&str>,
) -> Result<Arc<dyn clax_bot::WebSource>, String> {
    let own =
        clax_bot::web::OwnHosts::new(public_origin, site_domain).map_err(|e| e.to_string())?;
    Ok(if own == clax_bot::web::OwnHosts::default() {
        source
    } else {
        Arc::new(clax_bot::web::WithOwnHosts::new(source, own))
    })
}

/// `web`, the Worker's web source or why it has none: the source (and no
/// reason), or none and the reason, logged, which the state carries in
/// `clax_http::bot::Bot::web_check` for the admin Web view.
#[must_use]
pub fn web_or_reason(
    web: Result<Option<Arc<dyn clax_bot::WebSource>>, String>,
) -> (Option<Arc<dyn clax_bot::WebSource>>, Option<String>) {
    match web {
        Ok(web) => (web, None),
        Err(why) => {
            #[cfg(target_arch = "wasm32")]
            worker::console_error!("ClaxBot's web tools are off: {why}");
            (None, Some(why))
        }
    }
}

/// The `AI` binding's source, when the Worker has one.
fn binding_source(
    env: &worker::Env,
    public_origin: Option<&str>,
    check: Arc<clax_bot::web::HostCheck>,
    state: &worker::State,
) -> Option<Arc<dyn clax_bot::WebSource>> {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let raw: &wasm_bindgen::JsValue = state.as_raw().as_ref();
        let keep = worker::State::from(
            raw.clone()
                .unchecked_into::<worker::worker_sys::DurableObjectState>(),
        );
        crate::web_source::BindingSource::from_env(env, public_origin, check, keep)
            .map(|b| Arc::new(b) as Arc<dyn clax_bot::WebSource>)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (env, public_origin, check, state);
        None
    }
}

/// ClaxBot's state for the routes on the Worker: the key-encryption key
/// from `CLAX_KEK`, no environment key, `enabled` only when the sandbox
/// could be made (else a send is refused `not_configured`), and `runs`.
#[must_use]
pub fn routes_bot(
    kek: Option<Kek>,
    enabled: bool,
    runs: Arc<dyn clax_http::bot::Runs>,
) -> clax_http::bot::Bot {
    clax_http::bot::Bot {
        kek,
        env_key: None,
        enabled,
        runs,
        ..Default::default()
    }
}

/// The server's [`clax_http::bot::Runs`] on the Worker: the daemon's seam
/// ([`clax_server::bot::DriverRuns`], which hands each start and cancel
/// to the driver at once), noting that one came, so the object wants its
/// alarm after the request.
pub struct Kicks {
    inner: clax_server::bot::DriverRuns,
    kicked: AtomicBool,
}

impl Kicks {
    #[must_use]
    pub fn new(inner: clax_server::bot::DriverRuns) -> Kicks {
        Kicks {
            inner,
            kicked: AtomicBool::new(false),
        }
    }

    /// Whether a start or cancel came since the last look.
    pub fn take(&self) -> bool {
        self.kicked.swap(false, Ordering::SeqCst)
    }
}

impl clax_http::bot::Runs for Kicks {
    fn started(&self, run: &str) {
        self.inner.started(run);
        self.kicked.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self, run: &str) {
        self.inner.cancelled(run);
        self.kicked.store(true, Ordering::SeqCst);
    }
}

/// The run settings on the Worker: the instance settings in the database
/// (the admin pages', [`clax_http::bot::Settings`] with no configured
/// overrides), with the hosted infrastructure lines charged.
pub struct StoredSettings {
    pub inner: clax_http::bot::Settings,
    /// Whether the runs have a workspace ([`workspace_settings`]).
    pub workspace: bool,
    /// The least a turn is started with, but the default, in the Worker's
    /// tests.
    pub min_turn_budget: Option<Duration>,
}

impl SettingsSource for StoredSettings {
    fn settings(&self) -> clax_core::Result<RunSettings> {
        let mut s = hosted_settings(self.inner.settings()?);
        if self.workspace {
            s = workspace_settings(s);
        }
        if let Some(least) = self.min_turn_budget {
            s.min_turn_budget = least;
        }
        Ok(s)
    }
}

/// `s` as the Worker runs it: Durable Object requests and Toolpath
/// storage charged per step.
#[must_use]
pub fn hosted_settings(s: RunSettings) -> RunSettings {
    RunSettings {
        infra_lines: true,
        ..s
    }
}

/// `s` on a Worker with a workspace: at most [`WORKSPACE_CONCURRENCY`] runs.
#[must_use]
pub fn workspace_settings(s: RunSettings) -> RunSettings {
    RunSettings {
        concurrency: s.concurrency.min(WORKSPACE_CONCURRENCY),
        ..s
    }
}

/// When the alarm is wanted for the driver, apart from the driver itself
/// (so its rules are tested natively): a wake wants it now unless an
/// alarm drives (which starts what is woken itself); while one drives it
/// is wanted [`RESUME_AFTER`] ahead; after it, at the driver's next wake.
/// Times are ms by the object's clock.
#[derive(Debug)]
pub struct Schedule {
    due: Cell<Option<f64>>,
    driving: Cell<bool>,
    /// How far ahead the alarm is kept while one drives.
    resume_after: Duration,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule::new(RESUME_AFTER)
    }
}

impl Schedule {
    /// A schedule that keeps the alarm `resume_after` ahead while one
    /// drives ([`RESUME_AFTER`] but in the Worker's tests).
    #[must_use]
    pub fn new(resume_after: Duration) -> Schedule {
        Schedule {
            due: Cell::new(None),
            driving: Cell::new(false),
            resume_after,
        }
    }

    /// How far ahead the alarm is kept while one drives.
    #[must_use]
    pub fn resume_after(&self) -> Duration {
        self.resume_after
    }

    /// A run was woken (started, cancelled, resumed) at `now_ms`.
    pub fn woken(&self, now_ms: f64) {
        if !self.driving.get() {
            self.due
                .set(Some(self.due.get().map_or(now_ms, |d| d.min(now_ms))));
        }
    }

    /// When the alarm is wanted, at `now_ms`.
    #[must_use]
    pub fn due(&self, now_ms: f64) -> Option<f64> {
        if self.driving.get() {
            Some(now_ms + self.resume_after.as_millis() as f64)
        } else {
            self.due.get()
        }
    }

    /// Whether the driver's work is due by `now_ms` and no alarm drives.
    #[must_use]
    pub fn is_due(&self, now_ms: f64) -> bool {
        !self.driving.get() && self.due.get().is_some_and(|at| at <= now_ms)
    }

    /// Whether an alarm drives now.
    #[must_use]
    pub fn driving(&self) -> bool {
        self.driving.get()
    }

    /// An alarm starts driving.
    pub fn begin(&self) {
        self.due.set(None);
        self.driving.set(true);
    }

    /// The alarm stopped driving at `now_ms`; the driver next has work in
    /// `next_wake` (`Some(ZERO)` while runs are queued).
    pub fn finished(&self, next_wake: Option<Duration>, now_ms: f64) {
        self.driving.set(false);
        let at = next_wake.map(|d| now_ms + d.as_millis().min(u128::from(u32::MAX)) as f64);
        self.due.set(match (self.due.get(), at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        });
    }
}

/// ClaxBot's driver in the instance object (see the module docs).
pub struct WorkerBot {
    driver: Arc<Driver>,
    kicks: Arc<Kicks>,
    settings: Arc<StoredSettings>,
    schedule: Schedule,
    /// Whether [`Driver::start`] ran (at the incarnation's first event).
    started: Cell<bool>,
    host: Arc<clax_server::bot::ServerHost>,
    workspace: Option<Arc<WorkerWorkspace>>,
    /// When the workspace's timed work was last done (ms).
    last_tend: Cell<f64>,
    /// How often it is done.
    tend_every: Duration,
    /// How long an alarm drives, but the default, in the Worker's tests.
    alarm_budget: Option<Duration>,
}

/// What [`WorkerBot::new`] is made of: the parts the daemon's
/// `clax_server::bot::driver` makes its driver of.
pub struct Parts {
    pub store: Arc<Store>,
    pub sandbox: Arc<dyn Sandbox>,
    pub host: Arc<clax_server::bot::ServerHost>,
    /// The runs' containers, the same as the host's ([`crate::workspace`]);
    /// none: their API and prompt say nothing of one.
    pub workspace: Option<Arc<WorkerWorkspace>>,
    /// Whether the instance runs ClaxBot at all (its sandbox could be made).
    pub enabled: bool,
    pub keys: Keys,
    pub events: Arc<EventHub>,
    /// Where the runs' `clax.web` calls go ([`web_source`]); none: every
    /// web call a run makes is refused.
    pub web: Option<Arc<dyn clax_bot::WebSource>>,
    /// How far ahead the alarm is kept while one drives ([`RESUME_AFTER`]
    /// but in the Worker's tests).
    pub resume_after: Duration,
    /// Test settings, none of them on a deploy.
    pub tests: TestTimes,
}

/// Times a Worker's test shortens, so what takes minutes takes seconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct TestTimes {
    /// How long an alarm drives; turns then need a quarter of it.
    pub alarm_budget: Option<Duration>,
    /// How often the workspace's timed work is done.
    pub tend_every: Option<Duration>,
}

impl WorkerBot {
    /// A driver of `parts` with a fresh [`DriverId`], and the run seam the
    /// routes are to tell.
    #[must_use]
    pub fn new(parts: Parts) -> (WorkerBot, Arc<Kicks>) {
        let settings = Arc::new(StoredSettings {
            inner: clax_http::bot::Settings {
                store: parts.store.clone(),
                enabled: parts.enabled,
            },
            workspace: parts.workspace.is_some(),
            min_turn_budget: parts.tests.alarm_budget.map(|b| b / 4),
        });
        let host: Arc<dyn BotHost> = parts.host.clone();
        let deps = Deps::new(
            parts.store.clone(),
            MessagesClient::new(),
            parts.sandbox,
            host,
            settings.clone(),
            parts.keys,
            Arc::new(clax_server::bot::StoreClock(parts.store)),
        );
        let deps = match parts.web {
            Some(web) => deps.with_web(web),
            None => deps,
        };
        let deps = match parts.workspace.clone() {
            Some(w) => deps.with_lapsing_workspace(w),
            None => deps,
        };
        let driver = Arc::new(Driver::new(DriverId::mint(), Arc::new(deps), parts.events));
        let kicks = Arc::new(Kicks::new(clax_server::bot::DriverRuns::of(driver.clone())));
        let bot = WorkerBot {
            driver,
            kicks: kicks.clone(),
            settings,
            schedule: Schedule::new(parts.resume_after),
            started: Cell::new(false),
            host: parts.host,
            workspace: parts.workspace,
            last_tend: Cell::new(f64::NEG_INFINITY),
            tend_every: parts.tests.tend_every.unwrap_or(TEND_EVERY),
            alarm_budget: parts.tests.alarm_budget,
        };
        (bot, kicks)
    }

    /// The driver's ID (this incarnation's).
    #[must_use]
    pub fn driver_id(&self) -> &DriverId {
        self.driver.id()
    }

    /// The incarnation's first event starts the driver ([`Driver::start`]:
    /// the orphan sweep, then every run not ended, its claim taken back);
    /// the runs it queued want the alarm now.
    pub async fn start(&self, now_ms: f64) {
        if self.started.replace(true) {
            return;
        }
        match self.driver.start(true).await {
            Ok(0) => {}
            Ok(_) => self.schedule.woken(now_ms),
            Err(e) => {
                worker::console_error!("ClaxBot could not resume its runs: {e}");
                self.started.set(false);
            }
        }
    }

    /// Follows up the reports changed since the last call
    /// ([`clax_server::bot::ServerHost::follow_reports`]): what it starts
    /// [`WorkerBot::take_wakes`] then finds.
    pub async fn follow_reports(&self) {
        self.host.follow_reports().await;
    }

    /// When the workspace next has timed work (ms); `None` with none.
    #[must_use]
    pub fn next_tend(&self) -> Option<f64> {
        let w = self.workspace.as_ref()?;
        let held = w.tend_held_until_ms(self.tend_every);
        w.has_work()
            .then(|| tend_due(self.last_tend.get(), self.tend_every, held))
    }

    async fn tend_now(&self, now_ms: f64, deadline: Duration) {
        if let Some(w) = &self.workspace {
            self.last_tend.set(now_ms);
            w.tend(deadline).await;
        }
    }

    /// The workspace's timed work, when that is due.
    pub async fn tend(&self, now_ms: f64) {
        if self.next_tend().is_some_and(|at| at <= now_ms) {
            self.tend_now(now_ms, TEND_DEADLINE).await;
        }
    }

    /// Before a drive reads offers, unless the work was done since `since`
    /// (ms): an idle Worker's have lapsed.
    pub async fn tend_before_drive(&self, since: f64, now_ms: f64) {
        if self.last_tend.get() < since {
            self.tend_now(now_ms, TEND_DEADLINE).await;
        }
    }

    /// After a request that may have changed a report, before its follow-up
    /// reads an offer, when the workspace's word may have lapsed.
    pub async fn tend_for_a_request(&self, now_ms: f64) {
        let after = TEND_FOR_A_REQUEST_AFTER.as_millis() as f64;
        if now_ms - self.last_tend.get() >= after {
            self.tend_now(now_ms, REQUEST_TEND_DEADLINE).await;
        }
    }

    /// After a request: a start or cancel the routes told wants the alarm
    /// now, unless one drives.
    pub fn take_wakes(&self, now_ms: f64) {
        if self.kicks.take() {
            self.schedule.woken(now_ms);
        }
    }

    /// When the alarm is wanted for the driver ([`Schedule::due`]).
    #[must_use]
    pub fn due(&self, now_ms: f64) -> Option<f64> {
        self.schedule.due(now_ms)
    }

    /// [`Schedule::is_due`].
    #[must_use]
    pub fn is_due(&self, now_ms: f64) -> bool {
        self.schedule.is_due(now_ms)
    }

    /// The wall time this alarm drives for.
    #[must_use]
    pub fn budget(&self) -> Duration {
        if let Some(fixed) = self.alarm_budget {
            return fixed;
        }
        let min = self
            .settings
            .settings()
            .map_or(RunSettings::default().min_turn_budget, |s| {
                s.min_turn_budget
            });
        alarm_budget(min)
    }

    /// How far ahead the alarm is kept while one drives.
    #[must_use]
    pub fn resume_after(&self) -> Duration {
        self.schedule.resume_after()
    }

    /// Marks an alarm driving; [`WorkerBot::drive`] follows.
    pub fn begin(&self) {
        self.schedule.begin();
    }

    /// Ticks runs until idle or `budget` is spent, then records when the
    /// driver is next due; `now` reads the object's clock (ms).
    pub async fn drive(&self, budget: Duration, now: impl Fn() -> f64) {
        let report = self.driver.run_until_idle(budget).await;
        self.schedule.finished(report.next_wake, now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_source_names_the_instances_own_hosts() {
        let fixtures: Arc<dyn clax_bot::WebSource> =
            Arc::new(crate::web_source::FixtureWeb::default());
        let web = own_hosts(
            fixtures.clone(),
            Some("https://clax.example"),
            Some("sites.example"),
        )
        .unwrap();
        let own = web.own_hosts();
        assert!(own.holds("clax.example"));
        assert!(own.holds("abcdefghijkl.sites.example"));
        assert!(!own.holds("example.com"));
        assert_eq!(web.configured(), clax_bot::web::WebShape::BOTH);
        // Without either, the source is unchanged.
        let bare = own_hosts(fixtures.clone(), None, None).unwrap();
        assert_eq!(bare.own_hosts(), clax_bot::web::OwnHosts::default());
        // A set value that names no host is refused, not dropped.
        let e = own_hosts(fixtures.clone(), None, Some("https://sites.example/"))
            .err()
            .unwrap();
        assert!(e.contains("site domain"), "{e}");
        let e = own_hosts(fixtures, Some("not an origin"), None)
            .err()
            .unwrap();
        assert!(e.contains("public origin"), "{e}");
        // The Worker keeps starting: no source, and the reason in quotes.
        let (web, why) = web_or_reason(Err(e.clone()));
        assert!(web.is_none());
        assert_eq!(why, Some(e));
        assert!(web_or_reason(Ok(None)).1.is_none());
    }

    #[test]
    fn the_budget_leaves_the_margin_of_the_wall_limit_and_fits_a_turn() {
        let d = RunSettings::default().min_turn_budget;
        let b = alarm_budget(d);
        assert_eq!(b, Duration::from_secs(10 * 60));
        assert!(b >= d && b + ALARM_MARGIN <= ALARM_WALL_LIMIT);
        // A turn budget past it is given whole.
        assert_eq!(
            alarm_budget(Duration::from_secs(11 * 60)),
            Duration::from_secs(11 * 60)
        );
    }

    #[test]
    fn the_kek_is_64_hex_digits_and_unset_is_none() {
        assert!(kek_of("").unwrap().is_none());
        assert!(kek_of("  ").unwrap().is_none());
        let hex = "0123456789abcdefABCDEF0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(hex.len(), 64);
        assert!(kek_of(hex).unwrap().is_some());
        for bad in [
            &hex[..62],
            "zz23456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            let Err(e) = kek_of(bad) else {
                panic!("{} characters taken as a key", bad.len())
            };
            assert!(e.contains("CLAX_KEK") && !e.contains(bad), "{e}");
        }
        // Not a multi-byte character split across a pair.
        assert!(kek_of(&"é".repeat(32)).is_err());
    }

    #[test]
    fn hosted_settings_charge_the_infrastructure_lines() {
        assert!(!RunSettings::default().infra_lines);
        assert!(hosted_settings(RunSettings::default()).infra_lines);
    }

    #[test]
    fn a_worker_with_a_workspace_leaves_a_connection_for_its_poll() {
        let s = RunSettings {
            concurrency: 8,
            ..RunSettings::default()
        };
        assert_eq!(workspace_settings(s.clone()).concurrency, 5);
        assert_eq!(hosted_settings(s.clone()).concurrency, 8, "without one");
        let few = RunSettings {
            concurrency: 2,
            ..s
        };
        assert_eq!(workspace_settings(few).concurrency, 2);
    }

    #[test]
    fn without_its_sandbox_or_its_kek_claxbot_is_refused_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        let home = clax_core::Home::at(dir.path().to_path_buf());
        let store = Store::open(&home).unwrap();
        let owner = clax_store::roles::OWNER;
        let kek = || kek_of(&"ab".repeat(32)).unwrap();
        let runs = || -> Arc<dyn clax_http::bot::Runs> { Arc::new(clax_http::bot::NoRuns) };
        // No sandbox: off, whatever key is set.
        let off = routes_bot(kek(), false, runs());
        assert!(!off.enabled);
        assert_eq!(off.refusal(&store, owner).unwrap(), Some("not_configured"));
        // No key-encryption key: no instance key can be opened.
        let no_kek = routes_bot(None, true, runs());
        assert!(no_kek.kek().is_err());
        assert_eq!(
            no_kek.refusal(&store, owner).unwrap(),
            Some("not_configured")
        );
        assert!(routes_bot(kek(), true, runs()).env_key.is_none());
    }

    #[test]
    fn timed_work_held_back_waits_for_that_and_a_new_incarnation_does_too() {
        let every = Duration::from_secs(30);
        assert_eq!(tend_due(1_000.0, every, None), 31_000.0);
        assert_eq!(tend_due(1_000.0, every, Some(600_000)), 600_000.0);
        assert_eq!(tend_due(1_000.0, every, Some(2_000)), 31_000.0);
        assert_eq!(tend_due(f64::NEG_INFINITY, every, Some(600_000)), 600_000.0);
    }

    #[test]
    fn a_wake_wants_the_alarm_now_unless_one_drives() {
        let s = Schedule::default();
        assert_eq!(s.due(1_000.0), None);
        assert!(!s.is_due(1_000.0));
        s.woken(1_000.0);
        assert_eq!(s.due(5_000.0), Some(1_000.0));
        assert!(s.is_due(1_000.0) && !s.is_due(999.0));
        // A later wake keeps the earlier time.
        s.woken(2_000.0);
        assert_eq!(s.due(5_000.0), Some(1_000.0));
        // Driving: the alarm is kept RESUME_AFTER ahead, nothing is due to
        // a second alarm, and a wake is the driving alarm's to start.
        s.begin();
        assert!(s.driving());
        let ahead = RESUME_AFTER.as_millis() as f64;
        assert_eq!(s.due(10_000.0), Some(10_000.0 + ahead));
        assert_eq!(s.due(70_000.0), Some(70_000.0 + ahead));
        assert!(!s.is_due(1e12));
        s.woken(11_000.0);
        assert_eq!(s.due(12_000.0), Some(12_000.0 + ahead));
    }

    #[test]
    fn after_driving_the_alarm_is_wanted_at_the_drivers_next_wake() {
        let s = Schedule::default();
        s.begin();
        s.finished(Some(Duration::from_secs(30)), 1_000.0);
        assert!(!s.driving());
        assert_eq!(s.due(2_000.0), Some(31_000.0));
        // Runs still queued (the budget ran out): due at once.
        s.begin();
        s.finished(Some(Duration::ZERO), 40_000.0);
        assert_eq!(s.due(41_000.0), Some(40_000.0));
        assert!(s.is_due(40_000.0));
        // Nothing waits: no alarm for the driver.
        s.begin();
        s.finished(None, 50_000.0);
        assert_eq!(s.due(50_000.0), None);
        // A wake after the drive ended still wants the alarm now.
        s.woken(60_000.0);
        assert_eq!(s.due(60_000.0), Some(60_000.0));
        s.finished(Some(Duration::from_secs(5)), 61_000.0);
        assert_eq!(s.due(61_000.0), Some(60_000.0), "the earlier of the two");
    }

    #[test]
    fn a_far_wake_saturates_rather_than_overflows() {
        let s = Schedule::default();
        s.begin();
        s.finished(Some(Duration::MAX), 0.0);
        assert_eq!(s.due(0.0), Some(f64::from(u32::MAX)));
    }
}
