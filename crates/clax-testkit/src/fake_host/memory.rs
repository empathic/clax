//! [`MemoryHub`]: the run loop's hub of the host protocol
//! (`clax_bot::bot_surfaces`) in process, as a host's transport and as
//! the runs' side of the contract fixture's script.

use super::{HostClock, HostTransport, Names, ScriptWorld};
use clax_bot::bot_surfaces::{
    CallFailure, Enqueue, EnqueueError, Hub, HubLimits, MemoryPending, Millis, Ready, Retry, Tries,
};
use clax_bot::broker::stored::StoredPending;
use clax_bot::broker::{BrokerStore, Placed};
use clax_bot::surface::CallPlace;
use clax_store::bot_runs::{DriverId, StepKind};
use clax_surface::Reserved;
use clax_surface::wire::{
    self, End, ErrorBody, ErrorResponse, RegisterRequest, ServeRequest, ServeResponse,
    StartRunRequest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Inner {
    hub: Hub,
    store: Kept,
    registered: u64,
    started: Vec<StartRunRequest>,
}

/// The store a [`MemoryHub`] keeps its calls in.
enum Kept {
    /// In memory: [`MemoryPending`].
    Memory(MemoryPending),
    /// The store the runs are in ([`StoredPending`]), on a home of its
    /// own: a run's calls are opened under its claim, at a place of its
    /// first step, as a driver opens them.
    Stored(Box<Stored>),
}

/// [`Kept::Stored`]: the store, its home, the driver that holds the
/// runs, and each call's place by its key.
struct Stored {
    pending: StoredPending,
    db: std::path::PathBuf,
    _dir: tempfile::TempDir,
    driver: DriverId,
    places: BTreeMap<String, CallPlace>,
    /// Each run made, with the place its next new call takes.
    runs: BTreeMap<String, i64>,
}

impl Stored {
    fn new() -> Stored {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let home = clax_core::Home::at(dir.path().join("clax"));
        let store = std::sync::Arc::new(clax_store::Store::open(&home).expect("a store"));
        Stored {
            pending: StoredPending::new(store),
            db: home.db_path(),
            _dir: dir,
            driver: DriverId::mint(),
            places: BTreeMap::new(),
            runs: BTreeMap::new(),
        }
    }

    /// `call`, opened at its place of run `call.run`'s first step: the
    /// run is made, claimed and its step begun at its first call.
    fn enqueue(
        &mut self,
        hub: &mut Hub,
        registration: &str,
        call: Enqueue,
        now: Millis,
    ) -> Result<String, EnqueueError> {
        if !self.runs.contains_key(&call.run) {
            rusqlite::Connection::open(&self.db)
                .expect("the store's database")
                .execute(
                    "INSERT INTO bot_runs (id, artifact_id, person, request, thread_ids_json,
                        status, model, key_source, base_version, created_at, updated_at)
                     VALUES (?1, 'a', 'u', 'thread', '[]', 'queued', 'claude-sonnet-5-5',
                        'instance', 1, '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z')",
                    rusqlite::params![call.run],
                )
                .expect("the run is made");
            let st = self.pending.store().clone();
            st.claim_run(&call.run, &self.driver, 3_600)
                .expect("the run is claimed")
                .expect("the run is free");
            st.begin_step(&call.run, &self.driver, 1, StepKind::Script)
                .expect("its first step is begun");
            self.runs.insert(call.run.clone(), 0);
        }
        let place = if let Some(place) = self.places.get(&call.key) {
            place.clone()
        } else {
            let seq = self.runs.get_mut(&call.run).expect("the run is made");
            let place = CallPlace {
                step: 1,
                attempt: 1,
                seq: *seq,
                key: call.key.clone(),
            };
            *seq += 1;
            self.places.insert(call.key.clone(), place.clone());
            place
        };
        let raw = Value::Array(call.args.clone());
        let run = call.run.clone();
        let at = Placed {
            driver: &self.driver,
            run: &run,
            place: &place,
            raw: &raw,
            args_hash: clax_bot::host::args_hash(&raw),
            waits: false,
        };
        clax_bot::broker::enqueue_placed(hub, &mut self.pending, &at, registration, call, now)
    }
}

/// `$body` with `$store` the hub's store, whichever it is.
macro_rules! on_store {
    ($kept:expr, $store:ident => $body:expr) => {
        match $kept {
            Kept::Memory($store) => $body,
            Kept::Stored(stored) => {
                let $store = &mut stored.pending;
                $body
            }
        }
    };
}

/// A [`Hub`] and its [`MemoryPending`] at the time of a [`HostClock`].
/// Its `serve` waits for nothing: one that has nothing to send is
/// answered as the hub answers it when its wait is over, and the clock
/// is moved to then. Registration IDs are drawn in order
/// ([`registration_id`]). Its `runs` starts no run: it keeps the request and
/// answers `run-<n>`. Clones share the hub. Its runs' calls are sent
/// again as [`MemoryHub::retrying`] says: by default, never.
#[derive(Clone)]
pub struct MemoryHub {
    inner: Arc<Mutex<Inner>>,
    clock: HostClock,
    retry: Retry,
}

/// The `n`th registration ID this hub draws: `n` in 32 hex digits, so
/// the first is `…01` and none is the fixture's never-issued `0…0`.
#[must_use]
pub fn registration_id(n: u64) -> String {
    format!("{n:032x}")
}

fn refusal(body: ErrorBody) -> (u16, Value) {
    let status = wire::status(&body.code);
    let body = serde_json::to_value(ErrorResponse { error: body }).expect("JSON");
    (status, body)
}

impl MemoryHub {
    /// A hub with the default limits, as a daemon that keeps content has
    /// them (no operation withheld; a result that holds content is
    /// `invalid_result`, since nothing here fills it in), at `clock`'s
    /// time.
    #[must_use]
    pub fn new(clock: HostClock) -> MemoryHub {
        MemoryHub::with_limits(
            clock,
            HubLimits {
                content: true,
                ..HubLimits::default()
            },
        )
    }

    /// A hub with `limits`, at `clock`'s time.
    #[must_use]
    pub fn with_limits(clock: HostClock, limits: HubLimits) -> MemoryHub {
        MemoryHub::keeping(clock, limits, Kept::Memory(MemoryPending::new()))
    }

    /// [`MemoryHub::with_limits`], over the store the runs are in
    /// ([`StoredPending`]) on a home of its own, not [`MemoryPending`]:
    /// a call's attempts are kept in the store's columns, and a hub that
    /// starts again ([`MemoryHub::restart`]) reads them back from there.
    /// A run's calls are opened under its claim at their places, as its
    /// driver opens them; `run` names its row.
    #[must_use]
    pub fn in_the_store(clock: HostClock, limits: HubLimits) -> MemoryHub {
        MemoryHub::keeping(clock, limits, Kept::Stored(Box::new(Stored::new())))
    }

    fn keeping(clock: HostClock, limits: HubLimits, store: Kept) -> MemoryHub {
        let hub = Hub::new(limits, Reserved::of(&[]), Box::new(|_| {}));
        MemoryHub {
            retry: Retry::NONE,
            inner: Arc::new(Mutex::new(Inner {
                hub,
                store,
                registered: 0,
                started: Vec::new(),
            })),
            clock,
        }
    }

    #[must_use]
    pub fn clock(&self) -> &HostClock {
        &self.clock
    }

    /// This hub, whose runs' calls are sent again as `retry` says.
    #[must_use]
    pub fn retrying(mut self, retry: Retry) -> MemoryHub {
        self.retry = retry;
        self
    }

    /// The hub starts again as a daemon does: a new one, with `limits`,
    /// over the same store, holding no registration and nothing of
    /// what it held in memory. Clones share the new hub.
    pub fn restart(&self, limits: HubLimits) {
        let inner = &mut *self.inner.lock().unwrap();
        inner.hub = Hub::new(limits, Reserved::of(&[]), Box::new(|_| {}));
        // A store the runs are in is read again by a new adapter: nothing
        // the old one held in memory goes on.
        if let Kept::Stored(stored) = &mut inner.store {
            stored.pending = StoredPending::new(stored.pending.store().clone());
        }
    }

    fn now(&self) -> Millis {
        Millis(self.clock.now())
    }

    /// A run's call of `op` (its dotted name with the surface's:
    /// `desk.lamps`) for `registration`: the call's ID, or what its
    /// script is thrown.
    ///
    /// # Errors
    /// What the script is thrown.
    pub fn call(
        &self,
        registration: &str,
        run: &str,
        op: &str,
        args: Vec<Value>,
        key: &str,
        deadline_ms: Option<u64>,
    ) -> Result<String, CallFailure> {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        let call = Enqueue {
            run: run.to_string(),
            op: op.to_string(),
            args,
            key: key.to_string(),
            deadline: deadline_ms.map(Millis),
            retry: self.retry,
            joined: None,
        };
        let Inner { hub, store, .. } = inner;
        match store {
            Kept::Memory(store) => hub.enqueue(store, registration, call, now),
            Kept::Stored(stored) => stored.enqueue(hub, registration, call, now),
        }
        .map_err(|e| e.failure())
    }

    /// The call's outcome once it is settled, taken: `None` while it is
    /// open. Calls past their deadline are settled first.
    pub fn outcome(&self, id: &str) -> Option<Result<Value, CallFailure>> {
        let now = self.now();
        let Inner { hub, store, .. } = &mut *self.inner.lock().unwrap();
        on_store!(store, s => hub.expire(s, now).expect("the store"));
        match store {
            Kept::Memory(store) => store.take_answer(id),
            Kept::Stored(stored) => stored.pending.settled(id).expect("the store"),
        }
    }

    /// Whether the call is held and not settled.
    #[must_use]
    pub fn is_open(&self, id: &str) -> bool {
        let now = self.now();
        let Inner { hub, store, .. } = &mut *self.inner.lock().unwrap();
        on_store!(store, s => hub.expire(s, now).expect("the store"));
        match store {
            Kept::Memory(store) => store.is_open(id),
            Kept::Stored(stored) => stored
                .pending
                .store()
                .pending_call(id)
                .expect("the store")
                .is_some_and(|c| c.answered_at.is_none()),
        }
    }

    /// Records that `run`, of `registration`, ended: the notice's ID.
    ///
    /// # Panics
    /// When the hub refuses it.
    pub fn end(&self, registration: &str, run: &str, end: End, result: Option<Value>) -> String {
        let now = self.now();
        let Inner { hub, store, .. } = &mut *self.inner.lock().unwrap();
        on_store!(store, s => hub
            .ended(s, registration, run, end, result, now)
            .expect("the hub records the end"))
    }

    /// The runs' start requests, in order.
    #[must_use]
    pub fn started(&self) -> Vec<StartRunRequest> {
        self.inner.lock().unwrap().started.clone()
    }

    /// How many calls and unacknowledged ends the store holds.
    ///
    /// # Panics
    /// Over the store the runs are in, which counts no such thing.
    #[must_use]
    pub fn held(&self) -> (usize, usize) {
        match &self.inner.lock().unwrap().store {
            Kept::Memory(store) => store.held(),
            Kept::Stored(_) => panic!("held is counted in memory alone"),
        }
    }

    /// Where the open call `id` is with its attempts, as the store keeps
    /// it: `None` once it is settled.
    #[must_use]
    pub fn tries(&self, id: &str) -> Option<Tries> {
        match &self.inner.lock().unwrap().store {
            Kept::Memory(store) => store.tries_of(id).cloned(),
            Kept::Stored(stored) => stored.pending.tries(id).expect("the store"),
        }
    }

    /// Whether the hub dropped registration `id` for good, as its store
    /// keeps it.
    #[must_use]
    pub fn dropped(&self, id: &str) -> bool {
        let now = self.now();
        let Inner { hub, store, .. } = &mut *self.inner.lock().unwrap();
        on_store!(store, s => hub.dropped(s, id, now).expect("the store"))
    }
}

#[async_trait::async_trait]
impl HostTransport for MemoryHub {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        inner.registered += 1;
        let fresh = registration_id(inner.registered);
        let Inner { hub, store, .. } = inner;
        match on_store!(store, s => hub.register(s, name, body, &fresh, now)) {
            Ok(registered) => (200, serde_json::to_value(registered).expect("JSON")),
            Err(e) => refusal(e.body()),
        }
    }

    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        let now = self.now();
        let Inner { hub, store, .. } = &mut *self.inner.lock().unwrap();
        on_store!(store, store => self.serve_on(hub, store, name, body, now))
    }

    async fn start_run(&self, _name: &str, body: &StartRunRequest) -> (u16, Value) {
        let inner = &mut *self.inner.lock().unwrap();
        inner.started.push(body.clone());
        (
            200,
            json!({ "run": format!("run-{}", inner.started.len()) }),
        )
    }
}

impl MemoryHub {
    /// [`HostTransport::serve`] on `hub` over `store`.
    fn serve_on<P: clax_bot::bot_surfaces::Pending>(
        &self,
        hub: &mut Hub,
        store: &mut P,
        name: &str,
        body: &ServeRequest,
        now: Millis,
    ) -> (u16, Value) {
        let opened = match hub.open(
            store,
            name,
            &body.registration,
            body.wait_ms,
            body.answers.clone(),
            now,
        ) {
            Ok(opened) => opened,
            Err(e) => return refusal(e.body()),
        };
        // Nothing arrives while a request is held in one thread: one
        // that is not ready now is answered when its wait is over.
        let at = match hub.ready(store, &opened.serve, now) {
            Ok(Ready::Wait) => opened.serve.until(),
            Ok(Ready::Deliver | Ready::Ended) => now,
            Err(e) => return refusal(e.body()),
        };
        self.clock.set(self.clock.now().max(at.0));
        let ignored = opened.ignored_ids;
        match hub.close(store, opened.serve, at) {
            Ok(calls) => (
                200,
                serde_json::to_value(ServeResponse { calls, ignored }).expect("JSON"),
            ),
            Err(e) => refusal(e.body()),
        }
    }
}

/// The fixture's run steps, against the hub: `call` enqueues, `outcome`
/// and `waiting` read the store, `ended` records an end. Each step's
/// clock is set before it.
#[async_trait::async_trait]
impl ScriptWorld for MemoryHub {
    async fn at(&mut self, ms: u64) {
        self.clock.set(ms);
    }

    async fn run_step(&mut self, step: &Value, names: &mut Names) {
        let filled = |v: &Value, names: &mut Names| names.fill(v, step);
        match step["run"].as_str().unwrap_or("") {
            "call" => {
                let registration = filled(&step["registration"], names);
                let asked = &step["call"];
                let made = self.call(
                    registration.as_str().unwrap_or(""),
                    asked["run"].as_str().unwrap_or(""),
                    asked["op"].as_str().unwrap_or(""),
                    asked["args"].as_array().cloned().unwrap_or_default(),
                    asked["key"].as_str().unwrap_or(""),
                    asked["deadline_ms"].as_u64(),
                );
                match made {
                    Ok(id) => {
                        names.expect(
                            step["error"].is_null(),
                            "a call the script expects refused",
                            step,
                        );
                        names.hold(&step["id"], &json!(id), step);
                    }
                    Err(failure) => failed(names, &failure, step),
                }
            }
            "outcome" => {
                let id = filled(&step["id"], names);
                match self.outcome(id.as_str().unwrap_or("")) {
                    None => names.differ("the call is not settled", step),
                    Some(Ok(value)) => {
                        names.expect(
                            step["error"].is_null(),
                            "a value where an error is expected",
                            step,
                        );
                        names.expect(
                            step["result"] == value,
                            &format!("the result is {value}"),
                            step,
                        );
                    }
                    Some(Err(failure)) => failed(names, &failure, step),
                }
            }
            "waiting" => {
                let id = filled(&step["id"], names);
                let open = self.is_open(id.as_str().unwrap_or(""));
                names.expect(open, "the call is not waiting", step);
            }
            "ended" => {
                let registration = filled(&step["registration"], names);
                let ended = &step["ended"];
                let Ok(end) = serde_json::from_value(ended["end"].clone()) else {
                    return names.differ("the step's end is not an End", step);
                };
                let id = self.end(
                    registration.as_str().unwrap_or(""),
                    ended["run"].as_str().unwrap_or(""),
                    end,
                    ended.get("result").cloned(),
                );
                names.hold(&step["id"], &json!(id), step);
            }
            other => names.differ(
                &format!("a run step this world does not know: {other}"),
                step,
            ),
        }
    }
}

fn failed(names: &mut Names, failure: &CallFailure, step: &Value) {
    let want = &step["error"];
    names.expect(
        want["code"] == failure.code.as_str(),
        &format!("the script is thrown {failure:?}"),
        step,
    );
    names.expect(
        want["retryable"].as_bool().unwrap_or(false) == failure.retryable,
        &format!("retryable is {}", failure.retryable),
        step,
    );
}
