//! The daemon's side of the ClaxBot surfaces host protocol
//! (`docs/contract.md`, "ClaxBot surfaces"): the one broker a daemon
//! keeps ([`clax_bot::broker::HubBroker`]: the hub of registered surfaces
//! and its calls in the store, behind one lock), with what the routes
//! (`clax_server::routes::bot_surfaces`) do through it. The run loop's
//! side reaches the same broker as a [`clax_bot::broker::CallBroker`]
//! ([`Surfaces::broker`]), so a host's `serve` and a run's call meet in
//! one place.
//!
//! - [`Surfaces`] is the shared state ([`crate::state::AppState::bot_surfaces`]):
//!   the broker, a wake for each registration, and the timer's plan.
//!   Every method that lends the hub is synchronous and reads or writes
//!   the store under the broker's lock, so it is called on a store
//!   thread ([`clax_store::Store::call`]); the lock is never held across
//!   a wait.
//! - [`serve`] is one long-poll: `open`, a wait on the registration's
//!   wake or the request's time, `close`.
//! - [`run_timer`] settles calls at their deadlines and forgets old ends
//!   ([`Hub::expire`]) when [`Hub::next_wake`] says to.
//! - [`DaemonClock`] is the broker's clock: the store's (the system's in
//!   a daemon, a test's own in tests), in milliseconds since the Unix
//!   epoch, waiting on the surfaces' sleeper ([`Surfaces::set_sleeper`]).
//! - [`DaemonToken`] is who may use the protocol's routes.
//!
//! The hub's wake hook only notifies a registration's held `serve`s: it
//! is called under the broker's lock and never calls the broker back.
//! A run whose call an answer or a deadline settled is told to its
//! driver ([`wake_runs`]), as a run that was made is.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use clax_bot::bot_surfaces::{
    CODE_TOO_MANY_CHANGES, CODE_TOO_MANY_RUNS, FillContent, Hub, HubLimits, Millis, Ready,
    RegisterError, Registration, Serve, ServeError, Snapshot, check_answers_filled,
    is_registration_id, read_registration,
};
use clax_bot::broker::stored::StoredPending;
use clax_bot::broker::{BrokerClock, HubBroker};
use clax_bot::clax_surface::Reserved;
use clax_bot::clax_surface::wire::{
    self, Delivery, ErrorBody, RegisterRequest, Registered, ServeRequest, ServeResponse,
};
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_realtime::questions::Sleeper;
use clax_store::Store;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;

/// The daemon's broker: the hub over the store's calls.
pub type Broker = HubBroker<StoredPending>;

/// How long the timer waits before it looks again when the hub's next
/// wake is not after now though a pass was just made: a deadline that
/// has come and that the pass could not settle (the store failed). The
/// hub answers no other instant that is not after now
/// ([`Hub::next_wake`]).
pub const STALE_WAKE: Duration = Duration::from_secs(1);

/// Now, by the store's clock, as the hub counts instants: the Unix epoch
/// for an earlier one.
#[must_use]
pub fn now(store: &Store) -> Millis {
    Millis(u64::try_from(store.now_utc().timestamp_millis()).unwrap_or(0))
}

/// A new registration's ID: 128 bits from the operating system's random
/// source, as 32 lowercase hexadecimal digits.
#[must_use]
pub fn fresh_id() -> String {
    clax_core::secret::random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The sleeper the surfaces' waits run on, which a test replaces.
type SharedSleeper = Arc<Mutex<Arc<dyn Sleeper>>>;

fn sleeper_of(shared: &SharedSleeper) -> Arc<dyn Sleeper> {
    shared.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The daemon's [`BrokerClock`]: the store's clock, and waits on the
/// surfaces' sleeper. It is the clock the run loop's deps read
/// (`clax_server::bot`'s `StoreClock`, [`Store::now_utc`] too): a call's
/// deadline is counted by this one and a tick decides that it has passed
/// by that one, so the two must be one.
pub struct DaemonClock {
    store: Arc<Store>,
    sleeper: SharedSleeper,
}

#[async_trait::async_trait]
impl BrokerClock for DaemonClock {
    fn now(&self) -> Millis {
        now(&self.store)
    }

    async fn until(&self, at: Millis) {
        let left = at.since(self.now());
        if left > 0 {
            sleeper_of(&self.sleeper)
                .sleep(Duration::from_millis(left))
                .await;
        }
    }
}

/// Who registers a surface, for the registration's row and its audit
/// record ([`clax_store::Store::issue_registration`]).
#[derive(Clone, Debug)]
pub struct Registrant {
    pub ctx: AuditCtx,
    /// The person key the registration is made by.
    pub person: String,
}

// ---------------------------------------------------------------------------
// The shared state
// ---------------------------------------------------------------------------

/// The wake of each registration: what the hub's hook calls
/// ([`clax_bot::bot_surfaces::Wake`]) and a held `serve` waits on.
#[derive(Default)]
struct Wakes(Mutex<HashMap<String, Arc<Notify>>>);

/// What the hub's hook reaches: the registrations' wakes, and the
/// timer's, which plans again for whatever was just put in the store
/// for a host (a run's call with its deadline, a run's end).
#[derive(Default)]
struct Hooked {
    wakes: Wakes,
    /// Told when something the timer plans by may have changed.
    replan: Notify,
}

impl Wakes {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<Notify>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn of(&self, registration: &str) -> Arc<Notify> {
        self.lock()
            .entry(registration.to_string())
            .or_default()
            .clone()
    }

    fn wake(&self, registration: &str) {
        if let Some(wake) = self.lock().get(registration) {
            wake.notify_waiters();
        }
    }

    fn wake_all(&self) {
        for wake in self.lock().values() {
            wake.notify_waiters();
        }
    }

    /// Forgets the wakes no `serve` waits on.
    fn sweep(&self) {
        self.lock().retain(|_, wake| Arc::strong_count(wake) > 1);
    }
}

/// One registration as `GET /api/bot/surfaces` lists it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Listed {
    /// The surface's name.
    pub name: String,
    /// The registration's ID.
    pub registration: String,
    /// The hash of the definition it has now.
    pub hash: String,
    /// Whether a `serve` of it is open or closed lately
    /// ([`Hub::connected`]).
    pub connected: bool,
    /// The operations no run is given.
    pub withheld: Vec<wire::Withheld>,
    /// When it was first registered.
    pub registered_at: String,
    /// When a `serve` of it last closed; `null` before the first.
    pub last_served_at: Option<String>,
}

/// The daemon's broker of registered surfaces and what serves it: see
/// the module's documentation. One for each daemon
/// ([`crate::state::AppState::bot_surfaces`]); a registration is a lease
/// and ends with the process, and what outlasts it is in the store.
pub struct Surfaces {
    broker: Arc<Broker>,
    /// The hub's limits and reserved names, as it was made with them:
    /// what is read and checked with the hub let go is read and checked
    /// by these.
    limits: HubLimits,
    reserved: Reserved,
    store: Arc<Store>,
    hooked: Arc<Hooked>,
    sleeper: SharedSleeper,
    /// How many passes the timer has made ([`Surfaces::passes`]).
    passes: tokio::sync::watch::Sender<u64>,
    /// The uploads in flight, by registration ([`Surfaces::begin_upload`]).
    uploads: Arc<Mutex<HashMap<String, usize>>>,
}

/// One upload of a registration's in flight ([`Surfaces::begin_upload`]):
/// it is counted until this is dropped.
#[derive(Debug)]
pub struct Upload {
    uploads: Arc<Mutex<HashMap<String, usize>>>,
    registration: String,
}

impl Drop for Upload {
    fn drop(&mut self) {
        let mut uploads = self.uploads.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = uploads.get_mut(&self.registration) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                uploads.remove(&self.registration);
            }
        }
    }
}

/// The most bytes of JSON of the `meta` a provider gives a handle in an
/// answer.
pub const META_BYTES: usize = 4 * 1024;

/// What fills a result's references in on the daemon: the item of
/// `clax_store::bot_content` each names, when the call's run holds it
/// and the call's registration uploaded it; with the reference's `meta`.
/// The first answer that names an item records the call's operation,
/// key and step on its row.
struct StoreFill<'a> {
    store: &'a Store,
}

impl FillContent for StoreFill<'_> {
    fn fill(&self, call: &str, reference: &Value) -> Result<Value, String> {
        let st = self.store;
        let failed = |e: CoreError| {
            tracing::warn!("content was not read for the call {call}: {e}");
            "could not be read from the store".to_string()
        };
        let pending = st
            .pending_call(call)
            .map_err(failed)?
            .ok_or_else(|| "answers a call the daemon does not hold".to_string())?;
        let id = reference["id"].as_str().unwrap_or_default();
        let absent = || format!("names no content {id:?} that its host uploaded for the run");
        let row = st
            .bot_content_of(&pending.run_id, id)
            .map_err(failed)?
            .filter(|row| row.registration_id.as_deref() == Some(&pending.registration_id))
            .ok_or_else(absent)?;
        let meta = reference.get("meta").filter(|m| !m.is_null());
        if let Some(meta) = meta
            && meta.to_string().len() > META_BYTES
        {
            return Err(format!(
                "has a `meta` over {META_BYTES} bytes of JSON, which is the most a handle's has"
            ));
        }
        st.note_bot_content(id, &pending.op, &pending.key, pending.step_n, meta)
            .map_err(failed)?;
        let mut handle = row.handle();
        match meta {
            Some(meta) => handle["meta"] = meta.clone(),
            None => {
                if let Some(h) = handle.as_object_mut() {
                    h.remove("meta");
                }
            }
        }
        Ok(handle)
    }
}

/// The runs that made the calls `ids`, each once, in order.
fn runs_of(store: &Store, ids: &[String]) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    for id in ids {
        match store.pending_call(id) {
            Ok(Some(call)) if !runs.contains(&call.run_id) => runs.push(call.run_id),
            Ok(_) => {}
            Err(e) => tracing::warn!("the run of the settled call {id} was not read: {e}"),
        }
    }
    runs
}

impl Surfaces {
    /// A broker with no registration over `store`, within `limits`,
    /// waiting on `sleeper`. A host's surface may take no name of Clax's
    /// own ([`clax_bot::api::definition`]).
    #[must_use]
    pub fn new(store: Arc<Store>, sleeper: Arc<dyn Sleeper>, limits: HubLimits) -> Surfaces {
        let hooked = Arc::new(Hooked::default());
        let hook = hooked.clone();
        let sleeper: SharedSleeper = Arc::new(Mutex::new(sleeper));
        let reserved = Reserved::of(&[clax_bot::api::definition()]);
        let hub = Hub::new(
            limits.clone(),
            reserved.clone(),
            Box::new(move |registration| {
                hook.wakes.wake(registration);
                hook.replan.notify_one();
            }),
        );
        let clock = Arc::new(DaemonClock {
            store: store.clone(),
            sleeper: sleeper.clone(),
        });
        Surfaces {
            broker: Arc::new(HubBroker::new(
                hub,
                StoredPending::new(store.clone()),
                clock,
            )),
            limits,
            reserved,
            store,
            hooked,
            sleeper,
            passes: tokio::sync::watch::channel(0).0,
            uploads: Arc::default(),
        }
    }

    /// Counts one more upload of the registration `registration` in
    /// flight, while the [`Upload`] answered is held: `None`, and nothing
    /// counted, when it has [`HubLimits::uploads_per_registration`] in
    /// flight already.
    #[must_use]
    pub fn begin_upload(&self, registration: &str) -> Option<Upload> {
        let mut uploads = self.uploads.lock().unwrap_or_else(|e| e.into_inner());
        let n = uploads.entry(registration.to_string()).or_insert(0);
        if *n >= self.limits.uploads_per_registration {
            if *n == 0 {
                uploads.remove(registration);
            }
            return None;
        }
        *n += 1;
        Some(Upload {
            uploads: self.uploads.clone(),
            registration: registration.to_string(),
        })
    }

    /// The broker: what the run loop's side sends a run's calls through
    /// (as an `Arc<dyn clax_bot::broker::CallBroker>`, for
    /// [`clax_bot::outside::Registrations`]), and what lends the hub
    /// ([`HubBroker::with`]).
    #[must_use]
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Waits on `sleeper` from now on: the held `serve`s', the timer's
    /// and the broker's clock's.
    pub fn set_sleeper(&self, sleeper: Arc<dyn Sleeper>) {
        *self.sleeper.lock().unwrap_or_else(|e| e.into_inner()) = sleeper;
    }

    fn sleeper(&self) -> Arc<dyn Sleeper> {
        sleeper_of(&self.sleeper)
    }

    /// The hub's limits.
    #[must_use]
    pub fn limits(&self) -> HubLimits {
        self.limits.clone()
    }

    /// The passes the timer ([`run_timer`]) has made so far: a test waits
    /// on it for a pass to finish.
    #[must_use]
    pub fn passes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.passes.subscribe()
    }

    /// How many registrations have a wake kept for them: one for each
    /// that a `serve` is held for, and none for a request that was
    /// refused.
    #[must_use]
    pub fn wakes(&self) -> usize {
        self.hooked.wakes.lock().len()
    }

    /// Tells the timer that what it plans by may have changed. The hub
    /// tells it of every call and end put in the store for a host.
    pub fn replan(&self) {
        self.hooked.replan.notify_one();
    }

    /// Registers the surface `name` as `request` gives it, for
    /// `registrant`; a new registration's ID is `fresh` ([`fresh_id`]),
    /// and its row keeps the request's prompt file and settings. The
    /// definition and the prompt file are read before the broker is
    /// taken ([`read_registration`]), and the hub is given what they
    /// read to ([`Hub::register_parsed`]): no other host waits while
    /// one's files are read. Call it on a store thread.
    ///
    /// # Errors
    /// [`RegisterError`]: nothing was registered.
    pub fn register(
        &self,
        registrant: Registrant,
        name: &str,
        request: &RegisterRequest,
        fresh: &str,
    ) -> Result<Registered, RegisterError> {
        let settings = serde_json::to_value(&request.settings).unwrap_or(Value::Null);
        let reading = read_registration(&self.limits, &self.reserved, name, request)?;
        let registered = self.broker.with(|hub, store, now| {
            store.act_with(
                registrant.ctx,
                registrant.person,
                request.prompt.clone(),
                settings,
            );
            let registered = hub.register_parsed(store, reading, fresh, now);
            // The next registration is not this registrant's.
            store.act_with(
                AuditCtx::DAEMON,
                clax_store::roles::OWNER,
                None,
                Value::Null,
            );
            registered
        })?;
        self.replan();
        Ok(registered)
    }

    /// Whether the hub holds the registration `registration` of the
    /// surface `name` now ([`Hub::find`]).
    ///
    /// # Errors
    /// [`ServeError::UnknownRegistration`].
    pub fn find(&self, name: &str, registration: &str) -> Result<(), ServeError> {
        self.broker
            .with(|hub, _, now| hub.find(name, registration, now))
    }

    /// Whether a handle of `row`'s item was passed to the registration
    /// `registration`: one of the run's calls to it held, at a place its
    /// operation's declared type says is `Content` or `Image`
    /// ([`clax_bot::clax_surface::Operation::content_in_args`], read
    /// under the definition the call was sent under), a handle with the
    /// item's ID and SHA-256. A copy of a handle in an argument of type
    /// `unknown`, or in the `meta` of a handle passed, is at no such
    /// place, and passes nothing. Read from the calls the store keeps as
    /// they were sent ([`Store::bot_calls_naming`]); a call whose
    /// definition or operation is no longer known passes nothing. Call it
    /// on a store thread.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn passed(
        &self,
        row: &clax_store::bot_content::ContentRow,
        registration: &str,
    ) -> Result<bool, String> {
        let calls = self
            .store
            .bot_calls_naming(row, registration)
            .map_err(|e| e.to_string())?;
        for call in calls {
            let definition = self
                .broker
                .with(|hub, store, _| hub.kept_definition(store, &call.def_hash))?;
            let Some(operation) = definition.as_ref().and_then(|d| d.operation(&call.op)) else {
                continue;
            };
            let args = call.args.as_array().map_or(&[][..], Vec::as_slice);
            let Ok(places) = operation.content_in_args(args) else {
                continue;
            };
            let passed = places.iter().any(|place| {
                call.args.pointer(&place.at).is_some_and(|h| {
                    h["id"].as_str() == Some(row.id.as_str())
                        && h["sha256"].as_str() == Some(row.sha256.as_str())
                })
            });
            if passed {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// What `read` makes of the registration `id` (its name, definition,
    /// hash, prompt file, settings and withheld operations:
    /// [`Registration`]), when the hub holds it.
    pub fn registration<T>(&self, id: &str, read: impl FnOnce(Registration<'_>) -> T) -> Option<T> {
        self.broker.with(|hub, _, _| hub.registration(id).map(read))
    }

    /// The registration `id` as a copy of its own ([`Snapshot`]), when
    /// the hub holds it: what a request is checked against with the
    /// broker let go.
    #[must_use]
    pub fn snapshot(&self, id: &str) -> Option<Snapshot> {
        self.broker.with(|hub, _, _| hub.snapshot(id))
    }

    /// Opens a `serve` and, unless it is to wait, closes it: what became
    /// of it, and the runs whose calls its answers settled. The results
    /// and errors its answers bring are judged between two uses of the
    /// broker ([`Hub::checkable`], [`check_answers_filled`],
    /// [`Hub::open_checked`]), not under it; on a daemon that keeps
    /// content, a result's references are filled in from the store
    /// ([`StoreFill`]) on the way.
    fn begin(&self, name: &str, request: ServeRequest) -> Result<(Turn, Vec<String>), ServeError> {
        let checking = self.broker.with(|hub, store, now| {
            hub.checkable(store, name, &request.registration, &request.answers, now)
        })?;
        let fill = StoreFill { store: &self.store };
        let fill: Option<&dyn FillContent> = self.limits.content.then_some(&fill);
        let answers = check_answers_filled(&self.limits, request.answers, &checking, fill);
        self.broker.with(|hub, store, now| {
            let opened = hub.open_checked(
                store,
                name,
                &request.registration,
                request.wait_ms,
                answers,
                now,
            )?;
            let runs = runs_of(store.store(), &opened.settled);
            let turn = Self::turn(hub, store, opened.serve, now)?;
            Ok((turn, runs))
        })
    }

    /// What an open `serve` does now: waits on, or is closed and
    /// answered with what is due ([`Hub::ready`], then [`Hub::close`],
    /// under one hold of the hub), as it is once its wait is over.
    fn resume(&self, serve: Serve) -> Result<Turn, ServeError> {
        self.broker
            .with(|hub, store, now| Self::turn(hub, store, serve, now))
    }

    fn turn(
        hub: &mut Hub,
        store: &mut StoredPending,
        serve: Serve,
        now: Millis,
    ) -> Result<Turn, ServeError> {
        let until = serve.until();
        if hub.ready(store, &serve, now)? == Ready::Wait {
            return Ok(Turn::Wait(serve, until.since(now)));
        }
        let registration = serve.registration().to_string();
        let deliveries = hub.close(store, serve, now)?;
        store
            .store()
            .touch_registration(&registration, clax_bot::broker::stored::at(now))
            .map_err(|e| ServeError::Storage(e.to_string()))?;
        Ok(Turn::Answer(deliveries))
    }

    /// Closes a `serve` whose request went away ([`Hub::abandon`]): its
    /// registration's hold ends now, and no call is marked as sent to a
    /// host that was sent nothing.
    fn abandon(&self, serve: Serve) {
        self.broker.with(|hub, _, now| hub.abandon(serve, now));
    }

    /// One pass of the timer ([`Hub::expire`]): the calls whose deadline
    /// has come are settled and old ends forgotten; the runs whose calls
    /// it settled. Every held `serve` is woken, since a call its host
    /// asked for later may have come due. Call it on a store thread.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn expire(&self) -> Result<Vec<String>, String> {
        let runs = self.broker.with(|hub, store, now| {
            hub.expire(store, now)
                .map(|settled| runs_of(store.store(), &settled))
        })?;
        self.hooked.wakes.wake_all();
        self.hooked.wakes.sweep();
        Ok(runs)
    }

    /// When the timer next has something to do ([`Hub::next_wake`]), and
    /// the instant it is.
    ///
    /// # Errors
    /// The store's, as text.
    pub fn next_wake(&self) -> Result<(Option<Millis>, Millis), String> {
        self.broker
            .with(|hub, store, now| hub.next_wake(store, now).map(|next| (next, now)))
    }

    /// The registrations `person` made that the hub holds now, oldest
    /// first. Call it on a store thread.
    ///
    /// # Errors
    /// The store's.
    pub fn list(&self, person: &str) -> clax_core::Result<Vec<Listed>> {
        let rows = self.store.registrations_by(person, false)?;
        Ok(self.broker.with(|hub, _, now| {
            rows.into_iter()
                .filter_map(|row| {
                    let connected = hub.connected(&row.id, now);
                    let held = hub.registration(&row.id)?;
                    Some(Listed {
                        name: held.name.to_string(),
                        hash: held.hash.to_string(),
                        withheld: held.withheld.to_vec(),
                        connected,
                        registration: row.id,
                        registered_at: row.created_at,
                        last_served_at: row.last_served_at,
                    })
                })
                .collect()
        }))
    }
}

/// Tells the driver of each of `runs` that it has work: as a run that
/// was made is told ([`crate::bot::Runs::started`]).
pub fn wake_runs(s: &AppState, runs: &[String]) {
    for run in runs {
        s.bot.runs.started(run);
    }
}

// ---------------------------------------------------------------------------
// A serve
// ---------------------------------------------------------------------------

/// What an open `serve` is to do.
enum Turn {
    /// Wait this many milliseconds at the most, or until it is woken,
    /// and ask again.
    Wait(Serve, u64),
    /// It is closed: what its request is answered with.
    Answer(Vec<Delivery>),
}

/// Closes an open `serve` when it is dropped before [`serve`] closed it
/// (the request went away): the registration's hold ends then, not when
/// its wait would have.
struct Open {
    s: AppState,
    serve: Option<Serve>,
}

impl Drop for Open {
    fn drop(&mut self) {
        let Some(serve) = self.serve.take() else {
            return;
        };
        if !clax_rt::in_runtime() {
            return;
        }
        let s = self.s.clone();
        clax_rt::spawn(async move {
            let surfaces = s.bot_surfaces.clone();
            let closed = s
                .store
                .call(move |_| {
                    surfaces.abandon(serve);
                    Ok(())
                })
                .await;
            if let Err(e) = closed {
                tracing::warn!("a dropped serve was not closed: {e}");
            }
            s.bot_surfaces.replan();
        });
    }
}

/// Completes when the daemon begins to shut down; never, for a daemon
/// that cannot be told to.
async fn stopping(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// One `serve` of the surface `name` (`POST /api/bot/surfaces/<name>/serve`):
/// takes the request's answers, waits while nothing is new to the host
/// (for `wait_ms`, [`HubLimits::serve_wait_ms`] at the most, on the
/// surfaces' sleeper), and answers what the host is to do. The runs whose
/// calls it settled are told to their driver ([`wake_runs`]). Nothing is
/// held across the wait: the wake is listened for before the hub is asked
/// whether to wait, so a call that arrives between the two is not missed.
/// Each time the hub is asked, the `serve` is closed at the same instant
/// unless it is to wait on.
///
/// # Errors
/// [`ServeError`].
pub async fn serve(
    s: &AppState,
    name: String,
    request: ServeRequest,
) -> Result<ServeResponse, ServeError> {
    let surfaces = s.bot_surfaces.clone();
    let registration = request.registration.clone();
    // A wake is kept only for a registration the hub holds: a request
    // that names none leaves nothing behind.
    if !is_registration_id(&registration) {
        return Err(ServeError::UnknownRegistration);
    }
    surfaces.find(&name, &registration)?;
    // Made before the hub is asked for what is due: `notify_waiters`
    // wakes every future that exists when it runs.
    let wake = surfaces.hooked.wakes.of(&registration);
    let served = serve_woken(s, name, request, &wake).await;
    // The wakes no serve waits on are forgotten: this one's, unless
    // another serve of its registration is held.
    drop(wake);
    surfaces.hooked.wakes.sweep();
    served
}

/// [`serve`], once its registration is known to be held and has its
/// wake.
async fn serve_woken(
    s: &AppState,
    name: String,
    request: ServeRequest,
    wake: &Notify,
) -> Result<ServeResponse, ServeError> {
    let storage = |e: CoreError| ServeError::Storage(e.to_string());
    let surfaces = s.bot_surfaces.clone();
    let registration = request.registration.clone();
    let woken = wake.notified();
    tokio::pin!(woken);
    woken.as_mut().enable();
    let (turn, settled) = {
        let surfaces = surfaces.clone();
        s.store
            .call(move |_| Ok(surfaces.begin(&name, request)))
            .await
            .map_err(storage)??
    };
    wake_runs(s, &settled);
    // One more open `serve` than the hub allows ended the oldest, which
    // learns of it by asking; and the timer plans by this one's wait.
    surfaces.hooked.wakes.wake(&registration);
    surfaces.replan();
    let stopping = stopping(s.shutdown.clone());
    tokio::pin!(stopping);
    let mut turn = turn;
    let calls = loop {
        let (serve, left) = match turn {
            Turn::Answer(calls) => break calls,
            Turn::Wait(serve, left) => (serve, left),
        };
        let mut open = Open {
            s: s.clone(),
            serve: Some(serve),
        };
        let over = surfaces.sleeper().sleep(Duration::from_millis(left));
        tokio::select! {
            () = &mut woken => {}
            () = over => {}
            () = &mut stopping => {}
        }
        let stopped = *s.shutdown.borrow();
        woken.set(wake.notified());
        woken.as_mut().enable();
        let Some(serve) = open.serve.take() else {
            break Vec::new();
        };
        let asked = surfaces.clone();
        turn = s
            .store
            .call(move |_| Ok(asked.resume(serve)))
            .await
            .map_err(storage)??;
        if stopped && let Turn::Wait(serve, _) = turn {
            // The daemon is stopping: the request is answered now.
            let closing = surfaces.clone();
            let _ = s
                .store
                .call(move |_| {
                    closing.abandon(serve);
                    Ok(())
                })
                .await;
            break Vec::new();
        }
    };
    surfaces.replan();
    Ok(ServeResponse { calls })
}

// ---------------------------------------------------------------------------
// The timer
// ---------------------------------------------------------------------------

/// One pass of the timer now: [`Surfaces::expire`] on a store thread,
/// the settled calls' runs told to their driver, and the pass counted.
pub async fn pass(s: &AppState) {
    let surfaces = s.bot_surfaces.clone();
    let settled = s.store.call(move |_| Ok(surfaces.expire())).await;
    match settled {
        Ok(Ok(runs)) => wake_runs(s, &runs),
        Ok(Err(e)) => tracing::warn!("the surfaces' deadlines were not settled: {e}"),
        Err(e) => tracing::warn!("the surfaces' deadlines were not settled: {e}"),
    }
    s.bot_surfaces.passes.send_modify(|n| *n += 1);
}

/// The surfaces' timer: makes a [`pass`] whenever the hub's next wake
/// ([`Hub::next_wake`]: a call's deadline, a `serve` lapsing, a lease
/// ending, a call its host asked for later) comes, sleeping on the
/// surfaces' sleeper between, and plans again whenever a registration, a
/// `serve`, a call or an end changes what is next ([`Surfaces::replan`]).
/// Runs until its task is aborted; a daemon starts one
/// (`clax_server::core`), a Worker none.
pub async fn run_timer(s: AppState) {
    // Whether the last thing done was a pass that left the next wake in
    // the past (`STALE_WAKE`).
    let mut stale = false;
    loop {
        let surfaces = s.bot_surfaces.clone();
        let planned = s.store.call(move |_| Ok(surfaces.next_wake())).await;
        let wait = match planned {
            Ok(Ok((None, _))) => None,
            Ok(Ok((Some(next), at))) if next > at => Some(Duration::from_millis(next.since(at))),
            Ok(Ok((Some(_), _))) if stale => Some(STALE_WAKE),
            Ok(Ok((Some(_), _))) => Some(Duration::ZERO),
            Ok(Err(e)) => {
                tracing::warn!("the surfaces' timer could not plan: {e}");
                None
            }
            Err(e) => {
                tracing::warn!("the surfaces' timer could not plan: {e}");
                None
            }
        };
        match wait {
            None => {
                s.bot_surfaces.hooked.replan.notified().await;
                stale = false;
            }
            Some(d) if d.is_zero() => {
                pass(&s).await;
                stale = true;
            }
            Some(d) => {
                let over = s.bot_surfaces.sleeper().sleep(d);
                tokio::select! {
                    () = s.bot_surfaces.hooked.replan.notified() => stale = false,
                    () = over => {
                        pass(&s).await;
                        stale = true;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Who may, and how a refusal is written
// ---------------------------------------------------------------------------

/// The extractor of the host protocol's routes: the daemon's token and
/// nothing else. It admits a request only when
///
/// - the daemon is not hosted (a hosted daemon has no such routes: 404
///   `not_found`),
/// - it carries no agent credential of another machine (`Authorization:
///   Clax-Grant`, or an agent token `Bearer cxa_…`: 403 `forbidden`), and
/// - its `Authorization` header is `Bearer <the daemon's token>` (else
///   401 `unauthorized`).
///
/// So it admits whoever holds the token itself: the `clax` commands, a
/// host process on this machine, and the owner's agents (the token with a
/// session or the agent-channel marker is still the token). It does not
/// admit an owner browser session's cookie, an extension's credential, a
/// viewer's or a person's cookie, a `Clax-Grant` request whatever its
/// rights, or no credential. Its person is the owner
/// ([`DaemonToken::person`]).
pub struct DaemonToken;

impl DaemonToken {
    /// The person key the token's holder acts as: the owner's.
    #[must_use]
    pub fn person(&self) -> &'static str {
        clax_store::roles::OWNER
    }
}

impl FromRequestParts<AppState> for DaemonToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if state.hosted.is_some() {
            return Err(ApiError::not_found());
        }
        if crate::auth::is_grant_request(&parts.headers) {
            return Err(crate::auth::grants_refused());
        }
        if crate::auth::has_token(&parts.headers, &state.token) {
            Ok(DaemonToken)
        } else {
            Err(ApiError::unauthorized())
        }
    }
}

/// The codes of the protocol's refusals, as the errors' static codes.
const CODES: &[&str] = &[
    wire::CODE_UNKNOWN_REGISTRATION,
    wire::CODE_INVALID_REQUEST,
    wire::CODE_INVALID_DEFINITION,
    wire::CODE_UNSUPPORTED_VERSION,
    wire::CODE_NAME_MISMATCH,
    wire::CODE_INVALID_SETTINGS,
    wire::CODE_PROMPT_TOO_LARGE,
    wire::CODE_INVALID_PROMPT,
    wire::CODE_TOO_MANY_REGISTRATIONS,
    CODE_TOO_MANY_CHANGES,
    CODE_TOO_MANY_RUNS,
    wire::CODE_TOO_MANY_ANSWERS,
    wire::CODE_NOT_YET,
    wire::CODE_UNKNOWN_RUN,
    wire::CODE_TAKES_NO_MESSAGES,
    wire::CODE_RUN_ENDED,
    wire::CODE_KEY_CONFLICT,
    wire::CODE_TOO_MANY_MESSAGES,
    wire::CODE_TOO_LARGE,
    wire::CODE_INTERNAL_ERROR,
    wire::CODE_STORAGE_FAILED,
];

/// The status of a refusal's code: [`wire::status`], and 429 for the two
/// limits a host lifts by waiting that the hub adds
/// ([`CODE_TOO_MANY_CHANGES`], [`CODE_TOO_MANY_RUNS`]).
#[must_use]
pub fn status(code: &str) -> u16 {
    match code {
        CODE_TOO_MANY_CHANGES | CODE_TOO_MANY_RUNS => 429,
        other => wire::status(other),
    }
}

/// The refusal of a `serve` as the daemon answers it. Why a store
/// failed is logged here and is not in the answer.
#[must_use]
pub fn serve_refusal(e: &ServeError) -> ApiError {
    if let ServeError::Storage(why) = e {
        tracing::warn!("a serve met a store that failed: {why}");
    }
    refusal(e.body())
}

/// The refusal of a registration as the daemon answers it. Why a store
/// failed is logged here and is not in the answer.
#[must_use]
pub fn register_refusal(e: &RegisterError) -> ApiError {
    if let RegisterError::Storage(why) = e {
        tracing::warn!("a registration met a store that failed: {why}");
    }
    refusal(e.body())
}

/// A refusal of the protocol as the daemon answers it: the body
/// `{"error": <body>}` ([`wire::ErrorResponse`]) under the status its
/// code has ([`status`]). A code that is not one of the protocol's
/// is answered as `internal_error`.
#[must_use]
pub fn refusal(body: ErrorBody) -> ApiError {
    let code = CODES
        .iter()
        .copied()
        .find(|c| *c == body.code)
        .unwrap_or(wire::CODE_INTERNAL_ERROR);
    let status = StatusCode::from_u16(status(code)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut err = ApiError::new(status, code, body.message);
    if !body.errors.is_empty() {
        err.extra.insert("errors".into(), json!(body.errors));
    }
    if body.unread != 0 {
        err.extra.insert("unread".into(), json!(body.unread));
    }
    if !body.supported.is_empty() {
        err.extra.insert("supported".into(), json!(body.supported));
    }
    err
}

/// [`refusal`] with a code and a message alone.
#[must_use]
pub fn refused(code: &str, message: impl Into<String>) -> ApiError {
    refusal(ErrorBody::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_id_is_a_registration_id_and_no_other_fresh_one() {
        let (a, b) = (fresh_id(), fresh_id());
        assert!(clax_bot::bot_surfaces::is_registration_id(&a), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn a_refusal_has_its_codes_status_and_members() {
        let e = refusal(ErrorBody::unsupported_version(2));
        assert_eq!(e.status, StatusCode::BAD_REQUEST);
        assert_eq!(e.code, wire::CODE_UNSUPPORTED_VERSION);
        assert_eq!(e.extra["supported"], json!([1]));
        assert_eq!(
            refused(wire::CODE_UNKNOWN_REGISTRATION, "x").status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(refused(wire::CODE_TOO_LARGE, "x").status.as_u16(), 413);
        assert_eq!(refused(wire::CODE_NOT_YET, "x").status.as_u16(), 501);
        // A code the protocol does not have is not passed on.
        let other = refused("made_up", "x");
        assert_eq!(other.code, wire::CODE_INTERNAL_ERROR);
        assert_eq!(other.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
