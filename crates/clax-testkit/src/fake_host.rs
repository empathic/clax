//! A host of the ClaxBot surfaces protocol (`docs/contract.md`,
//! "ClaxBot surfaces"), for tests: [`FakeHost`] registers a surface,
//! answers the calls it is sent as each operation's [`Behaviour`] says,
//! acknowledges ended runs, starts runs, and keeps what it saw.
//!
//! It speaks through a [`HostTransport`], the protocol's three routes as
//! three methods: [`HttpTransport`] over a daemon's HTTP API, and
//! [`MemoryHub`] over the run loop's hub in process. Nothing here waits
//! on a clock: the test calls [`FakeHost::serve_once`] (or
//! [`FakeHost::serve_until`], bounded by rounds) and moves a
//! [`HostClock`] itself.
//!
//! [`replay`] replays the host steps of the contract fixture
//! (`docs/claxbot-surfaces/contract.json`) through any transport.
//!
//! A round of [`FakeHost::serve_once`]:
//!
//! 1. The answers that are due (an `accepted` call's result whose time
//!    has come, a late answer) join the ones kept from the round before.
//! 2. One `serve` brings them all.
//! 3. Each delivery is recorded. A call seen for the first time gets its
//!    operation's behaviour; an `ended` notice is acknowledged. Their
//!    answers go with the next round.

mod memory;
mod replay;
#[cfg(test)]
mod tests;

pub use memory::{MemoryHub, registration_id};
pub use replay::{Names, ScriptWorld, replay};

use clax_surface::wire::{
    Answer, CODE_UNKNOWN_REGISTRATION, CallError, Delivery, Notation, Outcome, RegisterRequest,
    Registered, ServeRequest, Settings, StartRunRequest, StartRunResponse, VERSION,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The protocol's three routes, as a host calls them: each answers the
/// HTTP status and the JSON body. A transport that got no answer at
/// all answers status 0 with an `ErrorResponse` body whose code is
/// `transport`.
#[async_trait::async_trait]
pub trait HostTransport: Send + Sync {
    /// `PUT /api/bot/surfaces/<name>`.
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value);
    /// `POST /api/bot/surfaces/<name>/serve`.
    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value);
    /// `POST /api/bot/surfaces/<name>/runs`.
    async fn start_run(&self, name: &str, body: &StartRunRequest) -> (u16, Value);
}

/// [`HostTransport`] over a daemon's HTTP API, with its bearer token.
#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base: String,
    token: String,
}

impl HttpTransport {
    /// To the daemon at `base` (`http://127.0.0.1:7480`), as `token`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> HttpTransport {
        HttpTransport {
            client: reqwest::Client::new(),
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    /// Sends `token` from now on: a daemon that restarted has a new one.
    pub fn set_token(&mut self, token: impl Into<String>) {
        self.token = token.into();
    }

    /// To a test daemon, with its token.
    pub fn of(ts: &crate::TestServer) -> HttpTransport {
        HttpTransport {
            client: ts.client.clone(),
            base: ts.base.clone(),
            token: ts.token.clone(),
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let sent = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await;
        match sent {
            Ok(res) => {
                let status = res.status().as_u16();
                let text = res.text().await.unwrap_or_default();
                let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
                (status, body)
            }
            Err(e) => (
                0,
                json!({"error": {"code": "transport", "message": e.to_string()}}),
            ),
        }
    }
}

#[async_trait::async_trait]
impl HostTransport for HttpTransport {
    async fn put_surface(&self, name: &str, body: &RegisterRequest) -> (u16, Value) {
        let path = format!("/api/bot/surfaces/{name}");
        self.send(reqwest::Method::PUT, &path, to_json(body)).await
    }
    async fn serve(&self, name: &str, body: &ServeRequest) -> (u16, Value) {
        let path = format!("/api/bot/surfaces/{name}/serve");
        self.send(reqwest::Method::POST, &path, to_json(body)).await
    }
    async fn start_run(&self, name: &str, body: &StartRunRequest) -> (u16, Value) {
        let path = format!("/api/bot/surfaces/{name}/runs");
        self.send(reqwest::Method::POST, &path, to_json(body)).await
    }
}

fn to_json(v: &impl serde::Serialize) -> Value {
    serde_json::to_value(v).expect("a protocol message is JSON")
}

/// A clock of milliseconds that only the test moves, shared by the
/// host and, in process, the hub ([`MemoryHub`]).
#[derive(Clone, Debug, Default)]
pub struct HostClock(Arc<AtomicU64>);

impl HostClock {
    /// A clock at `ms`.
    #[must_use]
    pub fn at(ms: u64) -> HostClock {
        HostClock(Arc::new(AtomicU64::new(ms)))
    }
    #[must_use]
    pub fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

/// When a late answer is sent.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    /// After this many more rounds that serve: `Rounds(2)` lets two
    /// rounds go by unanswered and answers with the third.
    Rounds(u32),
    /// Once the [`HostClock`] is this many milliseconds past the round
    /// that first delivered the call.
    AfterMs(u64),
}

/// How the host answers a call of one operation. `then` is a result
/// or an error ([`Outcome::Result`], [`Outcome::Error`]); a result of
/// the wrong type is a result like any other.
#[derive(Clone, Debug, PartialEq)]
pub enum Behaviour {
    /// Answers `outcome` with the next round.
    Now(Outcome),
    /// Answers `accepted` with the next round, and `then` when it is due.
    Accept { when: When, then: Outcome },
    /// Asks for the call again in `ms`; answers the next delivery of the
    /// call as `then` says.
    RetryAfter { ms: u64, then: Box<Behaviour> },
    /// Answers `then` when it is due, without `accepted` first: the
    /// daemon sends the call again with each round until then.
    Late { when: When, then: Outcome },
    /// Answers `outcome` with the next round and again with the one
    /// after: a second answer for a call that is settled.
    Twice(Outcome),
    /// Never answers.
    Never,
}

impl Behaviour {
    /// Answers `value` at once.
    pub fn result(value: Value) -> Behaviour {
        Behaviour::Now(Outcome::Result(value))
    }

    /// Fails at once with `code`.
    pub fn error(code: &str, message: &str) -> Behaviour {
        Behaviour::Now(Outcome::Error(CallError {
            code: code.to_string(),
            message: message.to_string(),
        }))
    }

    /// Fails at once with a code the operation does not declare
    /// (`@throws`): one the daemon settles as `invalid_result`.
    pub fn throws_undeclared() -> Behaviour {
        Behaviour::error("not_declared", "a code no @throws names")
    }

    /// A behaviour written as JSON, as the `fake_host` binary's answers
    /// file has them: `"never"`, `{"result": …}`, `{"error": {"code":
    /// …, "message": …}}`, or `{"accept": {"result": …}, "rounds": n}`.
    ///
    /// # Errors
    /// What is wrong with `v`.
    pub fn from_json(v: &Value) -> Result<Behaviour, String> {
        if v == "never" {
            return Ok(Behaviour::Never);
        }
        if let Some(then) = v.get("accept") {
            let rounds = v.get("rounds").and_then(Value::as_u64).unwrap_or(1);
            return Ok(Behaviour::Accept {
                when: When::Rounds(u32::try_from(rounds).map_err(|e| e.to_string())?),
                then: outcome_of(then)?,
            });
        }
        outcome_of(v).map(Behaviour::Now)
    }
}

fn outcome_of(v: &Value) -> Result<Outcome, String> {
    let mut answer = v.clone();
    answer["id"] = json!("_");
    serde_json::from_value::<Answer>(answer)
        .map(|a| a.outcome)
        .map_err(|e| format!("{v}: {e}"))
}

/// What a host that has not registered is answered, sending nothing.
fn unregistered() -> Value {
    json!({"error": {"code": "unregistered", "message": "register first"}})
}

/// Where the host is with one call it was delivered, by call ID.
#[derive(Debug)]
struct Progress {
    key: String,
    behaviour: Behaviour,
    /// The round that delivered the call to this behaviour, and the
    /// clock then.
    round: u64,
    at_ms: u64,
    /// Whether the call needs nothing more of the host.
    done: bool,
}

/// A host of one surface, for tests. See the module's documentation.
pub struct FakeHost<T> {
    transport: T,
    surface: String,
    request: RegisterRequest,
    registered: Option<Registered>,
    behaviours: BTreeMap<String, Behaviour>,
    otherwise: Behaviour,
    clock: HostClock,
    wait_ms: u64,
    rounds: u64,
    skip: u32,
    /// The answers for the next round.
    outbox: Vec<Answer>,
    progress: BTreeMap<String, Progress>,
    /// The outcome each key was applied with: a call delivered again
    /// under a key the host applied is answered the same, unapplied.
    outcomes: BTreeMap<String, Outcome>,
    applied: BTreeMap<String, u32>,
    acknowledged: BTreeSet<String>,
    deliveries: Vec<Delivery>,
}

/// What one round of [`FakeHost::serve_once`] came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Served {
    /// The round was one [`FakeHost::drop_rounds`] skipped: no request.
    Skipped,
    /// The daemon's deliveries.
    Delivered(Vec<Delivery>),
    /// Any other answer: the status and body. The round's answers are
    /// kept for the next.
    Refused(u16, Value),
}

impl<T: HostTransport> FakeHost<T> {
    /// A host of the surface `surface`, defined by `definition` in
    /// `notation`, that answers no call until told how
    /// ([`FakeHost::on`]).
    pub fn new(transport: T, surface: &str, notation: Notation, definition: &str) -> FakeHost<T> {
        FakeHost {
            transport,
            surface: surface.to_string(),
            request: RegisterRequest {
                clax_surface: VERSION,
                notation,
                definition: definition.to_string(),
                registration: None,
                prompt: None,
                settings: Settings::default(),
            },
            registered: None,
            behaviours: BTreeMap::new(),
            otherwise: Behaviour::Never,
            clock: HostClock::default(),
            wait_ms: 0,
            rounds: 0,
            skip: 0,
            outbox: Vec::new(),
            progress: BTreeMap::new(),
            outcomes: BTreeMap::new(),
            applied: BTreeMap::new(),
            acknowledged: BTreeSet::new(),
            deliveries: Vec::new(),
        }
    }

    /// Answers calls of `op` (its name below the surface, as a call
    /// names it: `settings.set`) as `behaviour` says.
    #[must_use]
    pub fn on(mut self, op: &str, behaviour: Behaviour) -> FakeHost<T> {
        self.behaviours.insert(op.to_string(), behaviour);
        self
    }

    /// Answers calls of the operations no [`FakeHost::on`] names as
    /// `behaviour` says (by default, [`Behaviour::Never`]).
    #[must_use]
    pub fn otherwise(mut self, behaviour: Behaviour) -> FakeHost<T> {
        self.otherwise = behaviour;
        self
    }

    /// Reads time from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: HostClock) -> FakeHost<T> {
        self.clock = clock;
        self
    }

    /// Asks the daemon to hold each `serve` up to `ms` (by default 0).
    #[must_use]
    pub fn wait_ms(mut self, ms: u64) -> FakeHost<T> {
        self.wait_ms = ms;
        self
    }

    /// Registers with this prompt file and these settings.
    #[must_use]
    pub fn with_registration(mut self, prompt: Option<&str>, settings: Settings) -> FakeHost<T> {
        self.request.prompt = prompt.map(str::to_string);
        self.request.settings = settings;
        self
    }

    /// Registers as `registration`, an earlier registration of this
    /// host's: a host that restarted goes on as the registration it was.
    #[must_use]
    pub fn resume(mut self, registration: &str) -> FakeHost<T> {
        self.request.registration = Some(registration.to_string());
        self
    }

    /// Registers the surface. A host that has a registration (its own,
    /// or one it [`FakeHost::resume`]s) goes on as it; when the daemon
    /// does not know it (`unknown_registration`), the host registers
    /// afresh.
    ///
    /// # Errors
    /// The daemon's status and body when it refused.
    pub async fn register(&mut self) -> Result<Registered, (u16, Value)> {
        let mut request = self.request.clone();
        if let Some(id) = self.registration() {
            request.registration = Some(id.to_string());
        }
        let (mut status, mut body) = self.transport.put_surface(&self.surface, &request).await;
        if request.registration.is_some() && body["error"]["code"] == CODE_UNKNOWN_REGISTRATION {
            request.registration = None;
            (status, body) = self.transport.put_surface(&self.surface, &request).await;
        }
        match serde_json::from_value::<Registered>(body.clone()) {
            Ok(registered) if status == 200 => {
                self.request.registration = None;
                self.registered = Some(registered.clone());
                Ok(registered)
            }
            _ => Err((status, body)),
        }
    }

    /// One round: see the module's documentation. A host that has not
    /// registered sends nothing and is refused with status 0, as is
    /// [`FakeHost::start_run`].
    pub async fn serve_once(&mut self) -> Served {
        let Some(registration) = self.registration().map(str::to_string) else {
            return Served::Refused(0, unregistered());
        };
        if self.skip > 0 {
            self.skip -= 1;
            return Served::Skipped;
        }
        self.rounds += 1;
        self.queue_due();
        let request = ServeRequest {
            registration,
            wait_ms: self.wait_ms,
            answers: std::mem::take(&mut self.outbox),
        };
        let (status, body) = self.transport.serve(&self.surface, &request).await;
        let calls = body.get("calls").cloned().unwrap_or(Value::Null);
        let deliveries = match serde_json::from_value::<Vec<Delivery>>(calls) {
            Ok(deliveries) if status == 200 => deliveries,
            _ => {
                self.outbox = request.answers;
                return Served::Refused(status, body);
            }
        };
        for delivery in &deliveries {
            self.see(delivery);
        }
        Served::Delivered(deliveries)
    }

    /// Serves round after round until `done` holds of the host, for
    /// `max_rounds` rounds at the most.
    ///
    /// # Errors
    /// The rounds served, when `done` never held.
    pub async fn serve_until(
        &mut self,
        mut done: impl FnMut(&FakeHost<T>) -> bool,
        max_rounds: u32,
    ) -> Result<u32, u32> {
        for round in 0..max_rounds {
            if done(self) {
                return Ok(round);
            }
            self.serve_once().await;
        }
        if done(self) {
            Ok(max_rounds)
        } else {
            Err(max_rounds)
        }
    }

    /// Sends no request for the next `n` rounds, as a host whose
    /// connection dropped; answers due meanwhile go with the round
    /// after.
    pub fn drop_rounds(&mut self, n: u32) {
        self.skip += n;
    }

    /// Sends `outcome` for the delivery `id` with the next round,
    /// whatever the host was delivered: an answer for an ID the
    /// daemon may not hold.
    pub fn answer(&mut self, id: &str, outcome: Outcome) {
        self.outbox.push(Answer {
            id: id.to_string(),
            outcome,
        });
    }

    /// Starts a run of the surface's kind `kind`, as this registration.
    ///
    /// # Errors
    /// The daemon's status and body when it refused.
    pub async fn start_run(
        &mut self,
        kind: &str,
        request: Value,
        allow: Option<Vec<String>>,
    ) -> Result<String, (u16, Value)> {
        let Some(registration) = self.registration().map(str::to_string) else {
            return Err((0, unregistered()));
        };
        let start = StartRunRequest {
            registration,
            kind: kind.to_string(),
            request,
            allow,
            bind: None,
        };
        let (status, body) = self.transport.start_run(&self.surface, &start).await;
        match serde_json::from_value::<StartRunResponse>(body.clone()) {
            Ok(started) if status == 200 => Ok(started.run),
            _ => Err((status, body)),
        }
    }

    /// How many times the host applied a call with `key`: once for
    /// each key it answered, however often the call came.
    #[must_use]
    pub fn applied(&self, key: &str) -> u32 {
        self.applied.get(key).copied().unwrap_or(0)
    }

    /// How many times the host applied each key ([`FakeHost::applied`]).
    #[must_use]
    pub fn applied_by_key(&self) -> &BTreeMap<String, u32> {
        &self.applied
    }

    /// Every delivery the host saw, in order, each time it saw it.
    #[must_use]
    pub fn deliveries(&self) -> &[Delivery] {
        &self.deliveries
    }

    /// How many times the delivery `id` came: a call is sent again until
    /// it is answered or accepted, and every open `serve` of a
    /// registration is sent what is due, so one ID may come often. The
    /// host acts on it once.
    #[must_use]
    pub fn times_seen(&self, id: &str) -> usize {
        self.deliveries
            .iter()
            .filter(|d| d.id() == Some(id))
            .count()
    }

    /// The deliveries of calls of `op`, each time one came.
    #[must_use]
    pub fn calls_of(&self, op: &str) -> Vec<&clax_surface::wire::Call> {
        self.deliveries
            .iter()
            .filter_map(|d| match d {
                Delivery::Call(c) if c.op == op => Some(c),
                _ => None,
            })
            .collect()
    }

    /// The `ended` notices, each time one came.
    #[must_use]
    pub fn ended(&self) -> Vec<&clax_surface::wire::Ended> {
        self.deliveries
            .iter()
            .filter_map(|d| match d {
                Delivery::Ended(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// The registration's ID, once registered.
    #[must_use]
    pub fn registration(&self) -> Option<&str> {
        self.registered.as_ref().map(|r| r.registration.as_str())
    }

    /// The daemon's answer to the last registration.
    #[must_use]
    pub fn registered(&self) -> Option<&Registered> {
        self.registered.as_ref()
    }

    /// The rounds that sent a request.
    #[must_use]
    pub fn rounds(&self) -> u64 {
        self.rounds
    }

    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport, to change how it reaches the daemon
    /// ([`HttpTransport::set_token`]).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    #[must_use]
    pub fn clock(&self) -> &HostClock {
        &self.clock
    }

    fn see(&mut self, delivery: &Delivery) {
        self.deliveries.push(delivery.clone());
        match delivery {
            Delivery::Call(call) => {
                if self.progress.contains_key(&call.id) {
                    // Delivered again: a call the host asked for later
                    // gets what follows; any other waits for its turn.
                    let progress = self.progress.get_mut(&call.id).expect("held");
                    if let Behaviour::RetryAfter { then, .. } = &progress.behaviour {
                        progress.behaviour = (**then).clone();
                        progress.round = self.rounds;
                        progress.at_ms = self.clock.now();
                        self.start(&call.id);
                    }
                    return;
                }
                let behaviour = match self.outcomes.get(&call.key) {
                    Some(outcome) => Behaviour::Now(outcome.clone()),
                    None => self
                        .behaviours
                        .get(&call.op)
                        .unwrap_or(&self.otherwise)
                        .clone(),
                };
                self.progress.insert(
                    call.id.clone(),
                    Progress {
                        key: call.key.clone(),
                        behaviour,
                        round: self.rounds,
                        at_ms: self.clock.now(),
                        done: false,
                    },
                );
                self.start(&call.id);
            }
            Delivery::Ended(ended) => {
                if self.acknowledged.insert(ended.id.clone()) {
                    self.answer(&ended.id, Outcome::Result(Value::Null));
                }
            }
            Delivery::Other(_) => {}
        }
    }

    /// What a call's behaviour sends with the round after the one that
    /// delivered it.
    fn start(&mut self, id: &str) {
        let progress = &self.progress[id];
        match progress.behaviour.clone() {
            Behaviour::Now(outcome) => self.settle(id, outcome),
            Behaviour::Twice(outcome) => {
                self.settle(id, outcome.clone());
                // The same answer again, with the round after.
                let progress = self.progress.get_mut(id).expect("held");
                progress.behaviour = Behaviour::Late {
                    when: When::Rounds(1),
                    then: outcome,
                };
                progress.done = false;
            }
            Behaviour::Accept { .. } => self.answer(id, Outcome::Accepted),
            Behaviour::RetryAfter { ms, .. } => self.answer(id, Outcome::RetryAfter(ms)),
            Behaviour::Late { .. } => {}
            Behaviour::Never => self.progress.get_mut(id).expect("held").done = true,
        }
    }

    /// Applies the call `id` with `outcome`, once per key, and answers.
    fn settle(&mut self, id: &str, outcome: Outcome) {
        let progress = self.progress.get_mut(id).expect("held");
        progress.done = true;
        let key = progress.key.clone();
        if !self.outcomes.contains_key(&key) {
            *self.applied.entry(key.clone()).or_default() += 1;
            self.outcomes.insert(key, outcome.clone());
        }
        self.answer(id, outcome);
    }

    /// Queues the late answers whose time has come.
    fn queue_due(&mut self) {
        let (round, now) = (self.rounds, self.clock.now());
        let due: Vec<(String, Outcome)> = self
            .progress
            .iter()
            .filter(|(_, p)| !p.done)
            .filter_map(|(id, p)| match &p.behaviour {
                Behaviour::Accept { when, then } | Behaviour::Late { when, then } => {
                    let due = match when {
                        When::Rounds(n) => round > p.round + u64::from(*n),
                        When::AfterMs(ms) => now >= p.at_ms.saturating_add(*ms),
                    };
                    due.then(|| (id.clone(), then.clone()))
                }
                _ => None,
            })
            .collect();
        for (id, outcome) in due {
            self.settle(&id, outcome);
        }
    }
}
