//! [`RemoteWorkspace`]: a [`Workspace`] whose containers are a service's.
//!
//! Each call is a job ([`crate::wire`]) that goes on whoever waits for it, so
//! the workspace is [`Workspace::resumable`]: the run loop may drop a call at
//! the end of its tick and join it in the next. What it waits on is short
//! polls, each bounded here, so a transport need bound nothing.
//!
//! A job known to be begun has no answer until the service gives one. While
//! the service cannot be reached it is waited for, with backoff, until the run
//! loop drops the wait: the tick ends and the step yields, and at the run's
//! time limit the run stops. Only the service's own [`CODE_NO_SUCH_JOB`] ends
//! the wait, and so does a refusal the service itself wrote
//! ([`crate::wire::REFUSALS`]), with its error: a wrong secret does not put
//! itself right. A proposal counts as begun from its first request, since its
//! key pushes nothing twice. A command that could not be sent fails closed at
//! once, with [`CODE_SHELL_UNAVAILABLE`].
//!
//! The words of every error of the transport's are fixed here, so what it
//! knows goes no further than it.
//!
//! [`Workspace::offer`] cannot wait: it answers from a [`Snapshot`], the
//! latest the service sent with any answer, for [`MAX_SNAPSHOT_AGE_MS`]. With
//! none, or an older one, the offers have lapsed ([`OffersLapse`]): every run
//! gets [`lapsed_offer`], which is no word of the service's, as
//! [`Offer::unsure`] is, and says nothing that lasts of any run. The lapse
//! reaches a run on a queue the last snapshot names, one it kept an offer for,
//! and with no snapshot any run on a queue: a run on no queue is no business of
//! the service's.
//!
//! The service's word on a link is not taken: a preview's and a pull
//! request's are kept only on the host's own [`LinkHosts`].
//!
//! What must outlive an incarnation of the host is [`Kept`], saved through a
//! [`Keeper`]. A host calls [`RemoteWorkspace::tend`] from its timed work, by
//! [`RemoteWorkspace::next_due_ms`], and at the start of any work that reads
//! offers. Tending that fails is held back, longer each time up to
//! [`LONGEST_TEND_PAUSE`], for as long as it keeps failing
//! ([`RemoteWorkspace::tend_held_until_ms`]), across incarnations too.

use crate::wire::{
    CODE_NO_SUCH_JOB, JOBS_PATH, JobAnswer, JobRequest, JobState, MAX_PREVIEW_EVENTS,
    MAX_RESPONSE_BYTES, PREVIEWS_ACK_PATH, PREVIEWS_PATH, PreviewTold, PreviewsAck, PreviewsAnswer,
    REAP_PATH, REFUSALS, ReapRequest, RunRef, STATUS_WIRE_SKEW, Snapshot, WIRE_HEADER,
    WIRE_INSTANCE_HEADER, WIRE_VERSION, WireFailure, Work, job_path, run_path,
};
use clax_bot::run::OffersLapse;
use clax_bot::workspace::{
    CODE_CHANGE_REFUSED, CODE_FIX_COMING, CODE_SHELL_UNAVAILABLE, Offer, PreviewSink, Proposal,
    Workspace, shell_interrupted,
};
use clax_bot_sandbox::{HostError, MaybeSendSync};
use clax_core::working::{Clock, SystemClock};
use clax_store::bot_runs::Run;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The longest a request is waited for: a poll's wait and the way there.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How many times a request is sent at once before it counts as failed.
/// Every request may be sent again ([`crate::wire`]).
const ATTEMPTS: u32 = 3;
/// The first and the longest pause while a begun job cannot be reached.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
const LONGEST_PAUSE: Duration = Duration::from_secs(30);
/// The least time between polls of a job, whatever a service does with `wait`.
const MIN_POLL: Duration = Duration::from_secs(1);
/// How many times a proposal the service lost, or that failed and may be
/// tried again, is sent again.
const RESENDS: u32 = 1;
/// How many polls may fail to tell a preview's word before it is given up.
pub const MAX_PREVIEW_TRIES: u32 = 20;
/// How old a snapshot may be and still be believed.
pub const MAX_SNAPSHOT_AGE_MS: i64 = 15 * 60 * 1000;
/// How old a snapshot is when [`RemoteWorkspace::tend`] asks for a new one.
const REFRESH_AFTER_MS: i64 = 5 * 60 * 1000;
/// How often the same snapshot is saved, for the age a new incarnation reads.
const SAVE_EVERY_MS: i64 = 60 * 1000;
/// How soon [`RemoteWorkspace::next_due_ms`] wants another
/// [`RemoteWorkspace::tend`] when there is work to tend.
const TEND_EVERY: Duration = Duration::from_secs(30);
/// The longest tending is held back, however long it has failed: less than
/// [`MAX_SNAPSHOT_AGE_MS`], so that offers come back soon after a service does.
pub const LONGEST_TEND_PAUSE: Duration = Duration::from_secs(10 * 60);

/// What every run gets while the offers have lapsed ([`OffersLapse`]): nothing
/// to work with, and confidential for as long as that lasts. It is not
/// [`Offer::unsure`], the service's word that a target could not be read.
#[must_use]
pub fn lapsed_offer() -> Offer {
    Offer::default().confidential()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// A request to send. The [`Transport`] adds the service's address and its
/// bearer secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireRequest {
    pub method: Method,
    /// The path and query.
    pub path: String,
    /// Every header it must carry.
    pub headers: Vec<(&'static str, String)>,
    /// JSON.
    pub body: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// No answer came. It says no more, so nothing a transport knows, a secret in
/// a header or an address, can reach a log or a script through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreachable;

/// How a host reaches its workspace service.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Transport: MaybeSendSync {
    /// Sends `request` and answers whatever status came back. Dropped, it
    /// sends nothing more. A body past
    /// [`crate::wire::MAX_RESPONSE_BYTES`] may be cut off or refused: it is
    /// not read.
    async fn send(&self, request: WireRequest) -> Result<WireResponse, Unreachable>;
}

/// What a [`RemoteWorkspace`] keeps across incarnations of its host, as JSON
/// or however a host likes. It holds no secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Kept {
    pub snapshot: Option<Snapshot>,
    /// When the snapshot came, in milliseconds since the epoch.
    pub received_ms: i64,
    /// When this was last saved for a snapshot that said nothing new.
    pub saved_ms: i64,
    /// Runs ended here whose ending the service has not answered.
    pub unfinished: BTreeSet<String>,
    /// How many polls failed to tell each preview event, by its ID.
    pub preview_tries: BTreeMap<String, u32>,
    /// The last poll of previews failed, so one is owed whatever
    /// [`Snapshot::previews_until_ms`] says.
    pub poll_failed: bool,
    /// How many tends in a row left work undone.
    pub tend_failures: u32,
    /// When the last of them ended, in milliseconds since the epoch.
    pub tend_failed_ms: i64,
}

/// Where a host keeps [`Kept`]. The host reads it back and hands it to
/// [`RemoteWorkspace::new`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Keeper: MaybeSendSync {
    /// Replaces what is kept.
    async fn save(&self, kept: &Kept);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostPattern {
    /// A host, or with `under` the zone below which hosts match.
    host: String,
    under: bool,
    port: Option<u16>,
}

/// Where a host allows a link to point, whatever a service says: `https` on a
/// host it names, at the port it names, or the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkHosts(Vec<HostPattern>);

impl LinkHosts {
    /// From comma-separated patterns, each a host, or `*.` before a zone with
    /// at least two labels, which stands for one or more whole labels below
    /// it; either may end in `:port`. A pattern that is neither is left out,
    /// and nothing allows no link at all.
    #[must_use]
    pub fn parse(patterns: &str) -> LinkHosts {
        let one = |p: &str| {
            let p = p.to_ascii_lowercase();
            let (name, port) = match p.rsplit_once(':') {
                Some((name, port)) => (name.to_string(), Some(port.parse().ok()?)),
                None => (p, None),
            };
            let (host, under) = match name.strip_prefix("*.") {
                Some(zone) => (zone.to_string(), true),
                None => (name, false),
            };
            let label = |l: &str| !l.is_empty() && !l.contains(['*', '/', '@', ' ']);
            let whole = host.split('.').all(label) && (!under || host.contains('.'));
            whole.then_some(HostPattern { host, under, port })
        };
        let patterns = patterns.split(',').map(str::trim);
        LinkHosts(patterns.filter(|p| !p.is_empty()).filter_map(one).collect())
    }

    /// Whether `link` may go to a person.
    #[must_use]
    pub fn allows(&self, link: &str) -> bool {
        let Ok(url) = url::Url::parse(link) else {
            return false;
        };
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
            return false;
        };
        let matches = |p: &HostPattern| {
            let name = if p.under {
                host.strip_suffix(p.host.as_str())
                    .and_then(|h| h.strip_suffix('.'))
                    .is_some_and(|h| !h.is_empty())
            } else {
                host == p.host
            };
            name && url.port() == p.port
        };
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && self.0.iter().any(matches)
    }
}

/// A [`Workspace`] over a [`Transport`]: see the module documentation.
pub struct RemoteWorkspace {
    transport: Arc<dyn Transport>,
    keeper: Arc<dyn Keeper>,
    instance: String,
    kept: Mutex<Kept>,
    sink: Mutex<Option<Arc<dyn PreviewSink>>>,
    preview_hosts: LinkHosts,
    forge_hosts: LinkHosts,
    clock: Arc<dyn Clock>,
}

fn unavailable(why: &str) -> HostError {
    HostError::new(CODE_SHELL_UNAVAILABLE, why)
}

fn unreachable_service() -> HostError {
    unavailable(
        "the workspace service could not be reached, so the state of what it was doing is \
         unknown",
    )
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn json<T: Serialize>(body: &T) -> Result<Vec<u8>, HostError> {
    serde_json::to_vec(body).map_err(|_| unavailable("a request could not be written"))
}

/// What `r`, which is not a success, says, in words of this crate's.
fn failed(r: &WireResponse) -> HostError {
    match r.status {
        STATUS_WIRE_SKEW => {
            let theirs = serde_json::from_slice::<WireFailure>(&r.body)
                .ok()
                .and_then(|f| f.wire)
                .map_or("another version".to_string(), |w| format!("version {w}"));
            unavailable(&format!(
                "the workspace service speaks wire {theirs} and this instance version \
                 {WIRE_VERSION}: one of them must be updated"
            ))
        }
        401 | 403 => unavailable("the workspace service refused this instance's credentials"),
        s => unavailable(&format!("the workspace service answered {s}")),
    }
}

/// `r`'s body, a success's.
fn read<T: DeserializeOwned>(r: &WireResponse) -> Result<T, HostError> {
    if !(200..300).contains(&r.status) {
        return Err(failed(r));
    }
    serde_json::from_slice(&r.body)
        .map_err(|_| unavailable("the workspace service's answer could not be read"))
}

/// Whether `r` is the service saying it has no such job.
fn no_such_job(r: &WireResponse) -> bool {
    r.status == 404
        && serde_json::from_slice::<WireFailure>(&r.body).is_ok_and(|f| f.code == CODE_NO_SUCH_JOB)
}

/// Whether `r` is the service refusing the request for good: a 4xx it wrote.
/// Anything else with that status is whoever stands on the way.
fn refused(r: &WireResponse) -> bool {
    (400..500).contains(&r.status)
        && serde_json::from_slice::<WireFailure>(&r.body)
            .is_ok_and(|f| REFUSALS.contains(&f.code.as_str()))
}

/// Waits out `pause`, and makes the next one longer.
async fn back_off(pause: &mut Duration) {
    clax_rt::time::sleep(*pause).await;
    *pause = (*pause * 2).min(LONGEST_PAUSE);
}

impl RemoteWorkspace {
    /// Over `transport`, for the instance `instance`, starting from `kept`,
    /// what `keeper` last saved, if anything. It allows no link until
    /// [`RemoteWorkspace::with_preview_hosts`] and
    /// [`RemoteWorkspace::with_forge_hosts`].
    #[must_use]
    pub fn new(
        transport: Arc<dyn Transport>,
        keeper: Arc<dyn Keeper>,
        kept: Option<Kept>,
        instance: impl Into<String>,
    ) -> RemoteWorkspace {
        RemoteWorkspace {
            transport,
            keeper,
            instance: instance.into(),
            kept: Mutex::new(kept.unwrap_or_default()),
            sink: Mutex::new(None),
            preview_hosts: LinkHosts::default(),
            forge_hosts: LinkHosts::default(),
            clock: Arc::new(SystemClock),
        }
    }

    /// This workspace, keeping preview links on `hosts`.
    #[must_use]
    pub fn with_preview_hosts(mut self, hosts: LinkHosts) -> RemoteWorkspace {
        self.preview_hosts = hosts;
        self
    }

    /// This workspace, keeping pull requests' links on `hosts`.
    #[must_use]
    pub fn with_forge_hosts(mut self, hosts: LinkHosts) -> RemoteWorkspace {
        self.forge_hosts = hosts;
        self
    }

    /// This workspace, reading the time from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> RemoteWorkspace {
        self.clock = clock;
        self
    }

    fn now_ms(&self) -> i64 {
        self.clock.now().timestamp_millis()
    }

    /// Until when a preview may yet be told of, in milliseconds since the
    /// epoch.
    #[must_use]
    pub fn previews_until_ms(&self) -> Option<i64> {
        let kept = lock(&self.kept);
        kept.snapshot.as_ref().and_then(|s| s.previews_until_ms)
    }

    /// Whether previews are to be polled at `now`.
    fn previews_due(&self, now: i64) -> bool {
        let kept = lock(&self.kept);
        let waiting = kept.snapshot.as_ref().and_then(|s| s.previews_until_ms);
        kept.poll_failed || !kept.preview_tries.is_empty() || waiting.is_some_and(|u| u > now)
    }

    /// When, in milliseconds since the epoch, [`RemoteWorkspace::tend`] has
    /// work to do; `None` while it has none, which a new run's offers do not
    /// change: a host tends before such work too.
    #[must_use]
    pub fn next_due_ms(&self) -> Option<i64> {
        let now = self.now_ms();
        let unfinished = !lock(&self.kept).unfinished.is_empty();
        let next = now + i64::try_from(TEND_EVERY.as_millis()).unwrap_or(i64::MAX);
        let held = self.tend_held_until_ms(TEND_EVERY).unwrap_or(next);
        (unfinished || self.previews_due(now)).then_some(next.max(held))
    }

    /// When tending is next worth trying, in milliseconds since the epoch, for
    /// a host that tends every `every`: `every` after the last failure, twice
    /// that after the one before it and so on, up to
    /// [`LONGEST_TEND_PAUSE`]. `None` when the last tend left nothing undone.
    #[must_use]
    pub fn tend_held_until_ms(&self, every: Duration) -> Option<i64> {
        let kept = lock(&self.kept);
        let doubled = kept.tend_failures.checked_sub(1)?.min(31);
        let pause = every.saturating_mul(1 << doubled).min(LONGEST_TEND_PAUSE);
        let pause = i64::try_from(pause.as_millis()).unwrap_or(i64::MAX);
        Some(kept.tend_failed_ms.saturating_add(pause))
    }

    /// Does what is due: brings an old snapshot up to date and polls previews,
    /// which come first, since a host may be waiting on the offers, and then
    /// retries the runs not yet ended.
    async fn tend_due(&self) {
        let now = self.now_ms();
        let old = {
            let kept = lock(&self.kept);
            kept.snapshot.is_none() || now - kept.received_ms >= REFRESH_AFTER_MS
        };
        if (old || self.previews_due(now))
            && let Err(e) = self.poll_previews().await
        {
            tracing::warn!(code = e.code, "previews could not be polled");
        }
        self.retry_finishes().await;
    }

    /// Notes how a tend went: `finished`, with nothing owed after it, clears
    /// what failed before it.
    async fn tended(&self, finished: bool) {
        let now = self.now_ms();
        let (owed, failures) = {
            let kept = lock(&self.kept);
            let owed = kept.poll_failed || !kept.unfinished.is_empty();
            (owed || !finished, kept.tend_failures)
        };
        if owed {
            self.change(|k| {
                k.tend_failures = failures.saturating_add(1);
                k.tend_failed_ms = now;
            })
            .await;
        } else if failures > 0 {
            self.change(|k| k.tend_failures = 0).await;
        }
    }

    /// Does what is due ([`RemoteWorkspace::tend_within`], with no deadline).
    pub async fn tend(&self) {
        self.tend_due().await;
        self.tended(true).await;
    }

    /// Does what is due, for at most `deadline`, and answers whether that was
    /// long enough. What it did not get to stays, and counts as failed.
    pub async fn tend_within(&self, deadline: Duration) -> bool {
        let finished = clax_rt::time::timeout(deadline, self.tend_due())
            .await
            .is_ok();
        self.tended(finished).await;
        finished
    }

    /// Changes what is kept, and saves it.
    async fn change(&self, f: impl FnOnce(&mut Kept)) {
        let kept = {
            let mut kept = lock(&self.kept);
            f(&mut kept);
            kept.clone()
        };
        self.keeper.save(&kept).await;
    }

    /// One sending of a request; `None` when no readable answer came.
    async fn once(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Option<WireResponse> {
        let request = WireRequest {
            method,
            path: path.to_string(),
            headers: vec![
                (WIRE_HEADER, WIRE_VERSION.to_string()),
                (WIRE_INSTANCE_HEADER, self.instance.clone()),
                ("content-type", "application/json".to_string()),
            ],
            body,
        };
        let sent = self.transport.send(request);
        match clax_rt::time::timeout(REQUEST_TIMEOUT, sent).await {
            Ok(Ok(r)) if r.body.len() <= MAX_RESPONSE_BYTES => Some(r),
            _ => None,
        }
    }

    /// Sends a request until an answer comes that is the service's to give.
    async fn ask(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<WireResponse, HostError> {
        self.ask_noting(method, path, body, &mut false).await
    }

    /// [`Self::ask`], setting `again` when the request went more than once.
    async fn ask_noting(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        again: &mut bool,
    ) -> Result<WireResponse, HostError> {
        let mut last = None;
        for n in 0..ATTEMPTS {
            *again |= n > 0;
            match self.once(method, path, body.clone()).await {
                Some(r) if r.status < 500 => return Ok(r),
                other => last = other,
            }
        }
        Err(last.as_ref().map_or_else(unreachable_service, failed))
    }

    /// Keeps `snapshot` as the latest word, unless a later one is here and
    /// not too old to stand against it.
    async fn keep(&self, snapshot: Snapshot) {
        let now = self.now_ms();
        let save = {
            let mut kept = lock(&self.kept);
            let fresh = now - kept.received_ms <= MAX_SNAPSHOT_AGE_MS;
            let held = kept.snapshot.as_ref();
            if held.is_some_and(|h| snapshot.seq < h.seq) && fresh {
                return;
            }
            let same = held.is_some_and(|h| h.says_the_same(&snapshot));
            kept.snapshot = Some(snapshot);
            kept.received_ms = now;
            let save = !same || now - kept.saved_ms >= SAVE_EVERY_MS;
            if save {
                kept.saved_ms = now;
            }
            save.then(|| kept.clone())
        };
        if let Some(kept) = save {
            self.keeper.save(&kept).await;
        }
    }

    /// What `answer` settles of its job, keeping its snapshot.
    async fn settled(&self, answer: JobAnswer) -> Option<Result<Value, HostError>> {
        self.keep(answer.snapshot).await;
        match answer.state {
            JobState::Running => None,
            JobState::Done { value } => Some(Ok(value)),
            JobState::Failed {
                code,
                message,
                retryable,
            } => {
                let e = HostError::new(code, message);
                Some(Err(if retryable { e.retryable() } else { e }))
            }
        }
    }

    /// Starts or joins `request`'s job. If `begun`, what is not the service's
    /// refusal is waited out. `again` is set when it went more than once.
    async fn start(
        &self,
        request: &JobRequest,
        begun: bool,
        again: &mut bool,
    ) -> Result<JobAnswer, HostError> {
        let body = json(request)?;
        let mut pause = FIRST_PAUSE;
        loop {
            let sent = self.ask_noting(Method::Post, JOBS_PATH, Some(body.clone()), again);
            let e = match sent.await {
                Ok(r) if refused(&r) => return Err(failed(&r)),
                Ok(r) => match read(&r) {
                    Ok(answer) => return Ok(answer),
                    Err(e) => e,
                },
                Err(e) => e,
            };
            if !begun {
                return Err(e);
            }
            tracing::warn!(key = request.key, "a begun job could not be sent");
            *again = true;
            back_off(&mut pause).await;
        }
    }

    /// Waits for job `key`, begun, for as long as it takes; `None` when the
    /// service says it has no such job, and its error when it refuses to say.
    async fn follow(&self, key: &str) -> Option<Result<Value, HostError>> {
        let path = job_path(key);
        let mut pause = FIRST_PAUSE;
        loop {
            let asked = clax_rt::time::Instant::now();
            let got = self.once(Method::Get, &path, None).await;
            if got.as_ref().is_some_and(no_such_job) {
                return None;
            }
            if let Some(r) = got.as_ref().filter(|r| refused(r)) {
                return Some(Err(failed(r)));
            }
            match got.as_ref().map(read::<JobAnswer>) {
                Some(Ok(answer)) => {
                    pause = FIRST_PAUSE;
                    if let Some(done) = self.settled(answer).await {
                        return Some(done);
                    }
                    let spent = asked.elapsed();
                    if spent < MIN_POLL {
                        clax_rt::time::sleep(MIN_POLL - spent).await;
                    }
                }
                // Not an answer about the job, whoever said it.
                _ => {
                    tracing::warn!(key, "a begun job could not be polled");
                    back_off(&mut pause).await;
                }
            }
        }
    }

    /// Starts or joins `request`'s job and waits for its answer. A command is
    /// never sent again once the service has lost it; a proposal is, as it
    /// is when it failed and may be tried again, since its key pushes nothing
    /// twice. A refusal of a proposal sent again speaks for that sending
    /// alone, so it is not answered as [`CODE_CHANGE_REFUSED`], which says
    /// nothing left: an earlier sending may have pushed.
    async fn job(&self, request: &JobRequest) -> Result<Value, HostError> {
        let proposal = matches!(request.work, Work::Propose { .. });
        let mut resent = 0;
        let mut again = false;
        loop {
            let answer = self.start(request, proposal, &mut again).await?;
            let done = match self.settled(answer).await {
                Some(done) => Some(done),
                None => self.follow(&request.key).await,
            };
            match done {
                Some(Err(e)) if proposal && e.retryable && resent < RESENDS => {}
                Some(Err(e)) if again && e.code == CODE_CHANGE_REFUSED => {
                    return Err(HostError::new(CODE_SHELL_UNAVAILABLE, e.message));
                }
                Some(done) => return done,
                None if !proposal => return Err(shell_interrupted()),
                None if resent < RESENDS => {}
                None => return Err(unavailable("the workspace service lost this proposal")),
            }
            resent += 1;
            again = true;
            clax_rt::time::sleep(FIRST_PAUSE).await;
        }
    }

    /// Tells the host of what the service has heard of its previews, and
    /// acknowledges what was told, at most
    /// [`crate::wire::MAX_PREVIEW_EVENTS`] of them; the rest is asked for
    /// again at the next poll. A link [`LinkHosts`] does not allow counts as
    /// no preview seen. A word that fails [`MAX_PREVIEW_TRIES`] polls is given up
    /// with a note to the members; a word waiting for its fix
    /// ([`CODE_FIX_COMING`]) just waits, as its run's end settles it. An
    /// answer for another instance is left alone. Answers how many were
    /// acknowledged.
    ///
    /// # Errors
    /// When the service cannot be reached, or speaks for another instance.
    pub async fn poll_previews(&self) -> Result<usize, HostError> {
        let Some(sink) = lock(&self.sink).clone() else {
            return Ok(0);
        };
        let polled = self.poll_previews_with(&sink).await;
        let failed = polled.is_err();
        if lock(&self.kept).poll_failed != failed {
            self.change(|k| k.poll_failed = failed).await;
        }
        polled
    }

    async fn poll_previews_with(&self, sink: &Arc<dyn PreviewSink>) -> Result<usize, HostError> {
        let r = self.ask(Method::Get, PREVIEWS_PATH, None).await?;
        let mut answer: PreviewsAnswer = read(&r)?;
        if answer.instance != self.instance {
            tracing::warn!("the workspace service answered for another instance");
            return Err(unavailable("the workspace service serves another instance"));
        }
        self.keep(answer.snapshot).await;
        answer.events.truncate(MAX_PREVIEW_EVENTS);
        let mut ids = Vec::new();
        let mut tries = lock(&self.kept).preview_tries.clone();
        for e in &answer.events {
            let told = match &e.told {
                PreviewTold::Ready {
                    verify_url,
                    reviewed,
                } if self.preview_hosts.allows(verify_url) => {
                    sink.preview_ready(&e.run, verify_url, *reviewed).await
                }
                PreviewTold::Ready { .. } => {
                    tracing::warn!(run = e.run, "a preview's link is on no allowed host");
                    sink.preview_missed(&e.run).await
                }
                PreviewTold::Missed => sink.preview_missed(&e.run).await,
            };
            match told {
                Ok(()) => ids.push(e.id.clone()),
                // The service spoke for this instance, so no such run here
                // means it is gone: there is nobody to tell, now or later.
                Err(err) if err.code == "not_found" => ids.push(e.id.clone()),
                Err(err) if err.code == CODE_FIX_COMING => {}
                Err(err) => {
                    tracing::warn!(run = e.run, code = err.code, "a preview was not told");
                    let n = tries.entry(e.id.clone()).or_insert(0);
                    *n += 1;
                    if *n >= MAX_PREVIEW_TRIES {
                        if let Err(lost) = sink.preview_lost(&e.run).await {
                            tracing::warn!(run = e.run, code = lost.code, "a preview was lost");
                        }
                        ids.push(e.id.clone());
                    }
                }
            }
        }
        tries.retain(|id, _| !ids.contains(id) && answer.events.iter().any(|e| e.id == *id));
        if tries != lock(&self.kept).preview_tries {
            self.change(|k| k.preview_tries = tries).await;
        }
        let told = ids.len();
        if told > 0 {
            let body = json(&PreviewsAck { ids })?;
            let r = self
                .ask(Method::Post, PREVIEWS_ACK_PATH, Some(body))
                .await?;
            if !(200..300).contains(&r.status) {
                return Err(failed(&r));
            }
        }
        Ok(told)
    }

    /// Asks the service to end run `id`, and forgets it once it has answered.
    async fn end(&self, id: &str) {
        let asked = async {
            let r = self.ask(Method::Delete, &run_path(id), None).await?;
            read::<Snapshot>(&r)
        };
        match asked.await {
            Ok(snapshot) => {
                self.keep(snapshot).await;
                self.change(|k| {
                    k.unfinished.remove(id);
                })
                .await;
            }
            Err(e) => tracing::warn!(run = id, code = e.code, "a run was not ended"),
        }
    }

    /// Asks again to end the runs whose ending the service has not answered.
    pub async fn retry_finishes(&self) {
        let ids = lock(&self.kept).unfinished.clone();
        for id in ids {
            self.end(&id).await;
        }
    }
}

impl OffersLapse for RemoteWorkspace {
    /// With no snapshot, or one past [`MAX_SNAPSHOT_AGE_MS`].
    fn offers_lapsed(&self) -> bool {
        let now = self.now_ms();
        let kept = lock(&self.kept);
        kept.snapshot.is_none() || now - kept.received_ms > MAX_SNAPSHOT_AGE_MS
    }

    /// By the last snapshot however old, which is word enough of what a queue
    /// has: with none, any queue may have a target.
    fn lapse_reaches(&self, run: &Run, queue: Option<&str>) -> bool {
        let kept = lock(&self.kept);
        match &kept.snapshot {
            Some(s) => {
                s.runs.contains_key(&run.id) || queue.is_some_and(|q| s.queues.contains_key(q))
            }
            None => queue.is_some(),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Workspace for RemoteWorkspace {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        let now = self.now_ms();
        let kept = lock(&self.kept);
        match &kept.snapshot {
            Some(s) if now - kept.received_ms <= MAX_SNAPSHOT_AGE_MS => s.offer(&run.id, queue),
            _ => lapsed_offer(),
        }
    }

    /// The service's word on it, in the snapshot: not its offer, which reads
    /// the same for a private target and one it could not read. With no
    /// snapshot, or an old one, not known to be private.
    fn target_private(&self, run: &Run, queue: Option<&str>) -> bool {
        let now = self.now_ms();
        let kept = lock(&self.kept);
        kept.snapshot.as_ref().is_some_and(|s| {
            now - kept.received_ms <= MAX_SNAPSHOT_AGE_MS && s.target_private(&run.id, queue)
        })
    }

    /// Refused: a job needs a key, and the run loop gives one
    /// ([`Workspace::exec_keyed`]).
    async fn exec(
        &self,
        _run: &Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        Err(unavailable("a command needs a key on this workspace"))
    }

    fn resumable(&self) -> bool {
        true
    }

    async fn exec_keyed(
        &self,
        run: &Run,
        queue: Option<&str>,
        key: &str,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.job(&JobRequest {
            key: key.to_string(),
            run: RunRef::of(run),
            work: Work::Exec {
                queue: queue.map(str::to_string),
                command: command.to_string(),
                cwd: cwd.map(str::to_string),
                timeout_secs,
            },
        })
        .await
    }

    async fn resume_exec(&self, _run: &Run, key: &str) -> Result<Option<Value>, HostError> {
        self.follow(key).await.transpose()
    }

    async fn propose(&self, run: &Run, proposal: &Proposal) -> Result<Value, HostError> {
        let mut done = self
            .job(&JobRequest {
                key: proposal.key.clone(),
                run: RunRef::of(run),
                work: Work::Propose {
                    title: proposal.title.clone(),
                    summary: proposal.summary.clone(),
                },
            })
            .await?;
        for (field, hosts) in [
            ("verify_url", &self.preview_hosts),
            ("url", &self.forge_hosts),
        ] {
            if done[field].as_str().is_some_and(|u| !hosts.allows(u)) {
                tracing::warn!(
                    run = run.id,
                    field,
                    "a proposal's link is on no allowed host"
                );
                done[field] = Value::Null;
            }
        }
        Ok(done)
    }

    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        *lock(&self.sink) = Some(sink);
    }

    async fn reap(&self, active: &[String]) {
        let asked = async {
            let body = json(&ReapRequest {
                active: active.to_vec(),
            })?;
            let r = self.ask(Method::Post, REAP_PATH, Some(body)).await?;
            read::<Snapshot>(&r)
        };
        match asked.await {
            Ok(snapshot) => {
                self.keep(snapshot).await;
                // What is not active was ended by this.
                if lock(&self.kept)
                    .unfinished
                    .iter()
                    .any(|r| !active.contains(r))
                {
                    self.change(|k| k.unfinished.retain(|r| active.contains(r)))
                        .await;
                }
            }
            Err(e) => tracing::warn!(code = e.code, "the workspace service did not reap"),
        }
    }

    async fn finish(&self, run: &Run) {
        self.change(|k| {
            k.unfinished.insert(run.id.clone());
        })
        .await;
        self.end(&run.id).await;
    }
}
