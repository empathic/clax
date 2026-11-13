//! [`MemoryHub`]: the run loop's hub of the host protocol
//! (`clax_bot::bot_surfaces`) in process, as a host's transport and as
//! the runs' side of the contract fixture's script.

use super::{HostClock, HostTransport, Names, ScriptWorld};
use clax_bot::bot_surfaces::{CallFailure, Enqueue, Hub, HubLimits, MemoryPending, Millis, Ready};
use clax_surface::Reserved;
use clax_surface::wire::{
    self, End, ErrorBody, ErrorResponse, RegisterRequest, ServeRequest, StartRunRequest,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Inner {
    hub: Hub,
    store: MemoryPending,
    registered: u64,
    started: Vec<StartRunRequest>,
}

/// A [`Hub`] and its [`MemoryPending`] at the time of a [`HostClock`].
/// Its `serve` waits for nothing: one that has nothing to send is
/// answered as the hub answers it when its wait is over, and the clock
/// is moved to then. Registration IDs are drawn in order
/// ([`registration_id`]). Its `runs` starts no run: it keeps the request and
/// answers `run-<n>`. Clones share the hub.
#[derive(Clone)]
pub struct MemoryHub {
    inner: Arc<Mutex<Inner>>,
    clock: HostClock,
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
    /// A hub with the default limits, at `clock`'s time.
    #[must_use]
    pub fn new(clock: HostClock) -> MemoryHub {
        let hub = Hub::new(HubLimits::default(), Reserved::of(&[]), Box::new(|_| {}));
        MemoryHub {
            inner: Arc::new(Mutex::new(Inner {
                hub,
                store: MemoryPending::new(),
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
        };
        inner
            .hub
            .enqueue(&mut inner.store, registration, call, now)
            .map_err(|e| e.failure())
    }

    /// The call's outcome once it is settled, taken: `None` while it is
    /// open. Calls past their deadline are settled first.
    pub fn outcome(&self, id: &str) -> Option<Result<Value, CallFailure>> {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        inner.hub.expire(&mut inner.store, now).expect("in memory");
        inner.store.take_answer(id)
    }

    /// Whether the call is held and not settled.
    #[must_use]
    pub fn is_open(&self, id: &str) -> bool {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        inner.hub.expire(&mut inner.store, now).expect("in memory");
        inner.store.is_open(id)
    }

    /// Records that `run`, of `registration`, ended: the notice's ID.
    ///
    /// # Panics
    /// When the hub refuses it.
    pub fn end(&self, registration: &str, run: &str, end: End, result: Option<Value>) -> String {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        inner
            .hub
            .ended(&mut inner.store, registration, run, end, result, now)
            .expect("the hub records the end")
    }

    /// The runs' start requests, in order.
    #[must_use]
    pub fn started(&self) -> Vec<StartRunRequest> {
        self.inner.lock().unwrap().started.clone()
    }

    /// How many calls and unacknowledged ends the store holds.
    #[must_use]
    pub fn held(&self) -> (usize, usize) {
        self.inner.lock().unwrap().store.held()
    }
}

#[async_trait::async_trait]
impl HostTransport for MemoryHub {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        inner.registered += 1;
        let fresh = registration_id(inner.registered);
        match inner
            .hub
            .register(&mut inner.store, name, body, &fresh, now)
        {
            Ok(registered) => (200, serde_json::to_value(registered).expect("JSON")),
            Err(e) => refusal(e.body()),
        }
    }

    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        let now = self.now();
        let inner = &mut *self.inner.lock().unwrap();
        let (hub, store) = (&mut inner.hub, &mut inner.store);
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
        match hub.close(store, opened.serve, at) {
            Ok(calls) => (200, json!({ "calls": calls })),
            Err(e) => refusal(e.body()),
        }
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
