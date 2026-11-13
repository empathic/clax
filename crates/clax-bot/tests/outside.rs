//! A surface a host registered, end to end in one process: the hub of
//! the host protocol and its calls in memory behind a broker
//! (`clax_bot::broker::HubBroker`), a host scripted by the test that
//! serves the registration through that hub, and runs whose bindings
//! name the registration, served by the outside provider
//! (`clax_bot::outside`) through the registry's resolver.
//!
//! The surface is the contract fixture's
//! (`docs/claxbot-surfaces/contract.json`, `desk.d.ts`) and the runs are
//! of its kind `desk.tidy`. Its request is `{ask: Instruction}`, which
//! a run can be started with today; `shell.triage` of the ClaxShell
//! example (`docs/superpowers/specs/2026-11-10-claxbot-surfaces/shell.d.ts`)
//! takes an `Image` in its request, and a request that holds content
//! cannot be made until Clax stores content. No file of the run loop's
//! sources names the surface ([`no_source_of_the_run_loop_names_the_fixtures_surface`]).
//!
//! The host is not a client of `clax-testkit`'s (`fake_host`): nothing
//! in this crate's tests depends on the testkit, which sits above the
//! daemon, and its hub in memory keeps its `Hub` to itself, so a broker
//! could not share it. [`Host`] serves the hub directly, with the
//! protocol's own messages.
//!
//! Nothing waits on a clock or on another task: the host takes its
//! turn whenever a run has sent a call ([`Served`]), and the broker's
//! clock moves only when a wait asks ([`SkipClock`]).

use crate::common::*;
use crate::generic::set_row;
use crate::resume::crash_once;
use clax_bot::bot_surfaces::{CallFailure, Hub, HubLimits, MemoryPending, Millis};
use clax_bot::broker::stored::StoredPending;
use clax_bot::broker::{
    BrokerStore, CallBroker, HubBroker, Outcome, Sending, Served as Registration, SkipClock,
};
use clax_bot::outside::{OutsideProvider, Registrations, narrowed};
use clax_bot::provider_conformance::{Maybe, OpCall, ProviderFixture, Waiting, check_provider};
use clax_bot::registry::{CODE_UNAVAILABLE, Providers};
use clax_bot::run::{CrashPoint, Deps};
use clax_bot::surface::{
    Binding, BindingAsStored, Bound, Claim, End, FAKE_SURFACE, RunInfo, StepCx, SurfaceProvider,
    dispatch,
};
use clax_bot::{DefaultPolicy, Delivery, Next, RunPolicy, Wait};
use clax_store::bot_runs::{Bindings, DriverId, RunStatus, StepKind, StoredBinding};
use clax_surface::Reserved;
use clax_surface::wire::{self, Answer, CallError, Notation, RegisterRequest, Settings};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FIXTURE: &str = include_str!("../../../docs/claxbot-surfaces/contract.json");

/// The fixture's surface file.
pub(crate) fn desk_dts() -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("the fixture is JSON");
    fixture["files"]["desk.d.ts"]
        .as_str()
        .expect("the fixture has desk.d.ts")
        .to_string()
}

/// What a scripted host does with a call: its answer, or `None` to
/// hold the call unanswered.
pub(crate) type Behave = Box<dyn FnMut(&wire::Call) -> Option<wire::Outcome> + Send>;

/// Which store the hub keeps its calls in: every test is played over
/// both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// In memory (`MemoryPending`): nothing is journaled but by the
    /// run loop, and a run's end is recorded by `ended`.
    InMemory,
    /// In the store the runs are in (`StoredPending`): an answer is
    /// journaled at its call's place when it arrives, and a run's end
    /// is kept by its terminal commit.
    InTheStore,
}

type Registers = Box<dyn Fn(&str, &RegisterRequest, &str) -> wire::Registered + Send + Sync>;
type Serves = Box<dyn Fn(&str, &str, Vec<Answer>) -> Vec<wire::Delivery> + Send + Sync>;
type Waits = Box<dyn Fn(&str) -> (usize, usize) + Send + Sync>;

/// The hub's side of a broker, whatever store it is over: what a host
/// and a test reach it by.
pub(crate) struct HubSide {
    pub(crate) register: Registers,
    serve: Serves,
    /// How many calls wait for a registration, and how many ends it has
    /// not acknowledged.
    waiting: Waits,
}

fn sides<S: BrokerStore + Send + 'static>(
    store: S,
    clock: Arc<SkipClock>,
) -> (Arc<dyn CallBroker>, Arc<HubSide>) {
    let hub = Hub::new(
        HubLimits::default(),
        Reserved::of(&[clax_bot::api::definition()]),
        Box::new(|_| {}),
    );
    let broker = Arc::new(HubBroker::new(hub, store, clock));
    let (a, b, c) = (broker.clone(), broker.clone(), broker.clone());
    let side = HubSide {
        register: Box::new(move |name, request, fresh| {
            a.with(|hub, store, now| hub.register(store, name, request, fresh, now))
                .expect("the surface registers")
        }),
        serve: Box::new(move |name, registration, answers| {
            b.with(|hub, store, now| hub.serve(store, name, registration, answers, now))
                .expect("the registration serves")
        }),
        waiting: Box::new(move |registration| {
            c.with(|_, store, _| {
                let pending = store
                    .pending_for(registration)
                    .unwrap_or_else(|e| panic!("what waits for {registration}: {e}"));
                (pending.calls.len(), pending.ends.len())
            })
        }),
    };
    (broker, Arc::new(side))
}

/// What a [`Host`] saw and did.
pub struct HostState {
    /// Each call it was delivered, once, in order.
    pub calls: Vec<wire::Call>,
    /// How many times it performed a call of each key.
    pub applied: BTreeMap<String, u32>,
    /// Each ended run it was told of, once.
    pub ended: Vec<wire::Ended>,
    seen: BTreeSet<String>,
    answers: Vec<Answer>,
    behave: Behave,
}

/// A host of one registration, scripted by its test: it serves the hub
/// when it is given a turn ([`Host::round`]), performs each call it is
/// delivered once, answers as its behaviour says and acknowledges each
/// ended run.
pub struct Host {
    side: Arc<HubSide>,
    name: String,
    pub registration: String,
    /// The hash of the definition it registered.
    pub hash: String,
    /// Whether it serves at all.
    pub online: AtomicBool,
    pub state: Mutex<HostState>,
}

pub(crate) fn result(value: Value) -> Option<wire::Outcome> {
    Some(wire::Outcome::Result(value))
}

fn error(code: &str, message: &str) -> Option<wire::Outcome> {
    Some(wire::Outcome::Error(CallError {
        code: code.into(),
        message: message.into(),
    }))
}

/// A request to register `definition`, a `.d.ts` file.
pub(crate) fn registering(
    definition: &str,
    registration: Option<String>,
    prompt: Option<&str>,
    settings: Settings,
) -> RegisterRequest {
    RegisterRequest {
        clax_surface: wire::VERSION,
        notation: Notation::Dts,
        definition: definition.to_string(),
        registration,
        prompt: prompt.map(str::to_string),
        settings,
    }
}

impl Host {
    /// Registers `request` as the surface `name` with the hub, as
    /// registration `n`.
    fn register(
        side: &Arc<HubSide>,
        name: &str,
        request: &RegisterRequest,
        n: u64,
        behave: Behave,
    ) -> Arc<Host> {
        let registered = (side.register)(name, request, &format!("{n:032x}"));
        Arc::new(Host {
            side: side.clone(),
            name: name.to_string(),
            registration: registered.registration,
            hash: registered.hash,
            online: AtomicBool::new(true),
            state: Mutex::new(HostState {
                calls: Vec::new(),
                applied: BTreeMap::new(),
                ended: Vec::new(),
                seen: BTreeSet::new(),
                answers: Vec::new(),
                behave,
            }),
        })
    }

    /// One `serve`, bringing the answers the host has ready; what it is
    /// delivered is performed and answered with the next.
    fn serve(&self) -> usize {
        let mut st = self.state.lock().unwrap();
        let answers = std::mem::take(&mut st.answers);
        let brought = answers.len();
        let deliveries = (self.side.serve)(&self.name, &self.registration, answers);
        for delivery in deliveries {
            match delivery {
                wire::Delivery::Call(call) => {
                    if !st.seen.insert(call.id.clone()) {
                        continue;
                    }
                    *st.applied.entry(call.key.clone()).or_default() += 1;
                    if let Some(outcome) = (st.behave)(&call) {
                        st.answers.push(Answer {
                            id: call.id.clone(),
                            outcome,
                        });
                    }
                    st.calls.push(call);
                }
                wire::Delivery::Ended(ended) => {
                    st.answers.push(Answer {
                        id: ended.id.clone(),
                        outcome: wire::Outcome::Result(Value::Null),
                    });
                    if st.seen.insert(ended.id.clone()) {
                        st.ended.push(ended);
                    }
                }
                wire::Delivery::Other(_) => {}
            }
        }
        brought + st.answers.len()
    }

    /// The host's turn: it serves until it has nothing to bring and is
    /// delivered nothing new. Nothing, for a host that is not online.
    pub fn round(&self) {
        if !self.online.load(Ordering::SeqCst) {
            return;
        }
        for _ in 0..8 {
            if self.serve() == 0 {
                return;
            }
        }
        panic!("the host and the hub did not settle in 8 serves");
    }

    /// Goes on as its registration, as a host that was away for longer
    /// than its lease does before it serves again.
    pub(crate) fn back(&self, definition: &str, fresh: u64) {
        let request = registering(
            definition,
            Some(self.registration.clone()),
            None,
            Settings::default(),
        );
        (self.side.register)(&self.name, &request, &format!("{fresh:032x}"));
    }

    /// Brings `outcome` for the call `id` with one `serve`: an answer
    /// that comes when the host chooses, not when it is asked.
    pub fn answer(&self, id: &str, outcome: wire::Outcome) {
        self.state.lock().unwrap().answers.push(Answer {
            id: id.to_string(),
            outcome,
        });
        self.serve();
    }

    /// How many times the host performed the call of `key`.
    pub fn applied(&self, key: &str) -> u32 {
        self.state
            .lock()
            .unwrap()
            .applied
            .get(key)
            .copied()
            .unwrap_or(0)
    }

    /// How many calls wait for the host, and how many ended runs it has
    /// not acknowledged.
    pub fn waiting(&self) -> (usize, usize) {
        (self.side.waiting)(&self.registration)
    }
}

/// The broker a run's side is given: the hub's, with the host taking
/// its turn each time a call was sent, as a host that long-polls would
/// between the call and its answer.
pub struct Served {
    inner: Arc<dyn CallBroker>,
    hosts: Mutex<Vec<Arc<Host>>>,
    /// While set, no prompt file can be read: the broker's store fails.
    prompts_unread: AtomicBool,
}

#[async_trait::async_trait]
impl CallBroker for Served {
    fn now(&self) -> Millis {
        self.inner.now()
    }
    fn registration(&self, id: &str, hash: Option<&str>) -> Option<Registration> {
        self.inner.registration(id, hash)
    }
    fn prompt(&self, hash: &str) -> Result<Option<String>, String> {
        if self.prompts_unread.load(Ordering::SeqCst) {
            return Err("disk I/O error".into());
        }
        self.inner.prompt(hash)
    }
    fn connected(&self, id: &str) -> bool {
        self.inner.connected(id)
    }
    fn send(&self, claim: &Claim<'_>, call: &Sending<'_>) -> Result<String, CallFailure> {
        let id = self.inner.send(claim, call)?;
        for host in self.hosts.lock().unwrap().iter() {
            host.round();
        }
        Ok(id)
    }
    async fn outcome(&self, call: &str, until: Millis) -> Outcome {
        self.inner.outcome(call, until).await
    }
    fn commits_ends(&self) -> bool {
        self.inner.commits_ends()
    }
    fn wake(&self, registration: &str) {
        self.inner.wake(registration);
    }
    fn ended(
        &self,
        registration: &str,
        run: &str,
        end: wire::End,
        result: Option<Value>,
    ) -> Result<String, String> {
        self.inner.ended(registration, run, end, result)
    }
}

/// A hub at a clock of its own that starts where the home's does, and
/// the broker over it that lets its hosts serve.
pub(crate) struct World {
    pub(crate) side: Arc<HubSide>,
    pub(crate) broker: Arc<Served>,
    pub(crate) clock: Arc<SkipClock>,
    started: u64,
}

impl World {
    fn new(env: &Env, kept: Kept) -> World {
        let started = u64::try_from(env.clock_now().timestamp_millis()).unwrap();
        let clock = Arc::new(SkipClock::at(started));
        let (inner, side) = match kept {
            Kept::InMemory => sides(MemoryPending::new(), clock.clone()),
            Kept::InTheStore => sides(StoredPending::new(env.store.clone()), clock.clone()),
        };
        let broker = Arc::new(Served {
            inner,
            hosts: Mutex::new(Vec::new()),
            prompts_unread: AtomicBool::new(false),
        });
        World {
            side,
            broker,
            clock,
            started,
        }
    }

    fn host(&self, name: &str, request: &RegisterRequest, behave: Behave) -> Arc<Host> {
        let n = self.broker.hosts.lock().unwrap().len() as u64 + 1;
        let host = Host::register(&self.side, name, request, n, behave);
        self.broker.hosts.lock().unwrap().push(host.clone());
        host
    }

    /// Moves the hub's clock and the home's on by `secs` seconds: time
    /// that passes for the daemon as a whole.
    pub(crate) fn pass(&self, env: &Env, secs: u64) {
        self.clock.advance(secs * 1000);
        env.clock.advance(i64::try_from(secs).unwrap());
    }

    /// The milliseconds the hub's clock has moved since it started.
    pub(crate) fn elapsed(&self) -> u64 {
        self.broker.now().0 - self.started
    }

    /// The resolver a daemon gives its registry, over this broker.
    pub(crate) fn resolver(&self, policy: Arc<dyn RunPolicy>) -> Arc<Registrations> {
        Arc::new(Registrations::new(self.broker.clone(), policy))
    }
}

// ---------------------------------------------------------------------------
// The fixture's surface, and runs of its kind
// ---------------------------------------------------------------------------

/// The lamps the desk's host answers.
pub(crate) fn lamps() -> Value {
    json!([{"id": "left", "on": true}, {"id": "right", "on": false}])
}

/// How the desk's host answers: `ask` is held unanswered.
pub(crate) fn desk_host() -> Behave {
    Box::new(|call| match call.op.as_str() {
        "lamps" => result(lamps()),
        "settings.set" | "notify" => result(Value::Null),
        "ask" => None,
        other => error("invalid", &format!("the desk has no {other}")),
    })
}

/// The operations a run of `desk.tidy` has: every one but the one that
/// answers an image.
pub(crate) const TIDYS: [&str; 4] = ["desk.lamps", "desk.settings.set", "desk.notify", "desk.ask"];

/// A home with the fixture's surface registered by a scripted host.
pub(crate) struct Desk {
    pub(crate) env: Env,
    pub(crate) world: World,
    pub(crate) host: Arc<Host>,
}

impl Desk {
    pub(crate) async fn new(kept: Kept) -> Desk {
        Desk::with(kept, Settings::default(), desk_host()).await
    }

    pub(crate) async fn with(kept: Kept, settings: Settings, behave: Behave) -> Desk {
        let env = Env::new().await;
        let world = World::new(&env, kept);
        let request = registering(&desk_dts(), None, None, settings);
        let host = world.host("desk", &request, behave);
        Desk { env, world, host }
    }

    /// Deps whose registry holds Clax's own surface by name, as a
    /// daemon's does, and resolves every other binding through the
    /// broker.
    pub(crate) fn deps(&self) -> Deps {
        self.deps_under(Arc::new(DefaultPolicy))
    }

    pub(crate) fn deps_under(&self, policy: Arc<dyn RunPolicy>) -> Deps {
        let mut deps = self.env.deps();
        deps.policy = policy.clone();
        deps.providers.set_resolver(self.world.resolver(policy));
        deps
    }

    /// A run of `desk.tidy` whose one binding is of the surface, served
    /// by the host's registration, with no hash.
    pub(crate) fn run(&self) -> String {
        self.run_bound(None, Some(&self.host.registration), &TIDYS)
    }

    /// A run of `desk.tidy` whose one binding, of version 2, is of the
    /// surface, with this hash, registration and operations: its row is
    /// made as a `clax` run's and given the kind, the request and the
    /// binding.
    pub(crate) fn run_bound(
        &self,
        hash: Option<&str>,
        registration: Option<&str>,
        ops: &[&str],
    ) -> String {
        self.run_started(hash, registration, None, ops)
    }

    /// [`Desk::run_bound`], started with the prompt file whose SHA-256
    /// is `prompt`: the binding a daemon's start route writes from a
    /// registration that has one.
    pub(crate) fn run_started(
        &self,
        hash: Option<&str>,
        registration: Option<&str>,
        prompt: Option<&str>,
        ops: &[&str],
    ) -> String {
        let binding = StoredBinding::bound(
            "desk",
            hash.map(str::to_string),
            registration.map(str::to_string),
            prompt.map(str::to_string),
            Value::Null,
            ops.iter().map(|op| (*op).to_string()).collect(),
        );
        let bindings = Bindings::bound(vec![binding]).expect("bindings of version 2");
        let run = self.env.run();
        set_row(
            &self.env,
            &run,
            "kind = 'desk.tidy', artifact_id = NULL, base_version = NULL, bindings_json = ?2, \
             request_json = ?3",
            &bindings.to_json(),
            &json!({"ask": "Turn the lamps down."}).to_string(),
        );
        run
    }

    pub(crate) fn journal(&self, run: &str, n: i64) -> Vec<Value> {
        self.env
            .store
            .bot_calls(run, n)
            .unwrap()
            .iter()
            .map(|c| json!({"seq": c.seq(), "op": c.op(), "ok": c.ok(), "result": c.result()}))
            .collect()
    }

    pub(crate) fn value(&self, run: &str, n: usize) -> Value {
        let steps = self.env.store.run_with_steps(run).unwrap().unwrap().1;
        steps[n].payload.as_ref().unwrap()["outcome"]["value"].clone()
    }
}

/// Plays `$case` over each store the hub can keep its calls in.
macro_rules! over_both_stores {
    ($($(#[$doc:meta])* $name:ident => $case:ident;)*) => {$(
        mod $name {
            $(#[$doc])*
            #[tokio::test]
            async fn in_memory() {
                super::$case(super::Kept::InMemory).await;
            }
            $(#[$doc])*
            #[tokio::test]
            async fn in_the_store() {
                super::$case(super::Kept::InTheStore).await;
            }
        }
    )*};
}

/// A script that reads the lamps and changes a setting.
const TIDY: &str = r#"const lamps = await clax.desk.lamps();
await clax.desk.settings.set({theme: "dark"});
return lamps;"#;

/// What a run of [`TIDY`] leaves with its host and in its journal.
fn check_tidy(d: &Desk, run: &str) {
    let st = d.host.state.lock().unwrap();
    let seen: Vec<Value> = st
        .calls
        .iter()
        .map(|c| {
            json!({"run": c.run, "op": c.op, "args": c.args, "key": c.key, "call": c.call,
                   "expires_in_ms": c.expires_in_ms})
        })
        .collect();
    // Each call once: its operation below the surface's name, its
    // arguments, the key of its place, what it is to its surface, and
    // the daemon's default deadline.
    assert_eq!(
        seen,
        [
            json!({"run": run, "op": "lamps", "args": [], "key": format!("{run}:2:0"),
                   "call": "read", "expires_in_ms": 30_000}),
            json!({"run": run, "op": "settings.set", "args": [{"theme": "dark"}],
                   "key": format!("{run}:2:1"), "call": "write", "expires_in_ms": 30_000}),
        ]
    );
    assert_eq!(
        st.applied.values().copied().collect::<Vec<_>>(),
        [1, 1],
        "each key was performed once"
    );
    drop(st);
    assert_eq!(
        d.journal(run, 2),
        [
            json!({"seq": 0, "op": "desk.lamps", "ok": true, "result": lamps()}),
            json!({"seq": 1, "op": "desk.settings.set", "ok": true, "result": null}),
        ]
    );
    assert_eq!(d.value(run, 1), lamps());
}

/// A run of the fixture's kind calls its host's operations from a
/// script and finishes: the host saw each call once, with its
/// operation, arguments, key and kind, the journal holds the answers,
/// and the run's end reaches the host with the kind's result and is
/// acknowledged.
async fn a_hosts_surface_answers_a_runs_calls_and_is_told_its_end(kept: Kept) {
    let d = Desk::new(kept).await;
    d.env.api.push(script_turn("tu_1", "Tidy", TIDY));
    d.env.api.push(finish_turn("The lamps are down."));
    let run = d.run();
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    check_tidy(&d, &run);
    // The script's `clax` had the surface's operations and Clax's own
    // none: the run has no `clax` binding.
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert_eq!(
        bound.operations(),
        [
            "desk.lamps",
            "desk.snapshot",
            "desk.settings.set",
            "desk.notify",
            "desk.ask"
        ]
    );
    // The run's own set leaves out the operation that answers an image.
    assert_eq!(
        bound.info.bindings[0].operations,
        ["desk.lamps", "desk.settings.set", "desk.notify", "desk.ask"]
    );
    assert_eq!(bound.info.bindings[0].hash, None);
    // The end is recorded for the registration and waits for its host.
    assert_eq!(d.host.waiting(), (0, 1));
    assert!(d.host.state.lock().unwrap().ended.is_empty());
    d.host.round();
    let st = d.host.state.lock().unwrap();
    let [ended] = &st.ended[..] else {
        panic!("{:?}", st.ended);
    };
    assert_eq!(ended.run, run);
    assert_eq!(ended.end, wire::End::Finished {});
    assert_eq!(
        ended.result,
        Some(json!({"summary": "The lamps are down."}))
    );
    drop(st);
    assert_eq!(d.host.waiting(), (0, 0), "the host acknowledged the end");
}

/// The driver dies before the script step's commit: the step run again
/// is answered from the journal, and the host is sent nothing more.
async fn a_hosts_calls_are_not_sent_again_when_a_crashed_step_is_run_again(kept: Kept) {
    let d = Desk::new(kept).await;
    d.env.api.push(script_turn("tu_1", "Tidy", TIDY));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let mut dying = d.deps();
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let out = d.env.drive(&dying, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    assert_eq!(d.host.state.lock().unwrap().calls.len(), 2);
    d.env.clock.advance(61);
    // The host is away by now: the replayed calls do not need it.
    d.host.online.store(false, Ordering::SeqCst);
    d.world.clock.advance(HubLimits::default().connected_ms + 1);
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    check_tidy(&d, &run);
    let steps = d.env.store.run_with_steps(&run).unwrap().unwrap().1;
    assert_eq!(steps[1].attempt, 2);
    assert_eq!(d.host.applied(&format!("{run}:2:0")), 1);
    assert_eq!(d.host.applied(&format!("{run}:2:1")), 1);
}

/// A script that calls `op` and returns what it was thrown.
fn thrown(call: &str) -> String {
    format!(
        "try {{ return [\"answered\", await {call}]; }} \
         catch (e) {{ return [e.code, e.message, e.retryable]; }}"
    )
}

/// A host that is not connected takes no call: the script is thrown
/// `unavailable`, which is journaled, and a host that answers a value
/// of another type than the operation declares is `invalid_result`.
async fn a_host_that_is_away_is_unavailable_and_a_wrong_answer_is_an_invalid_result(kept: Kept) {
    let wrong: Behave = Box::new(|_| result(json!("no lamps here")));
    let d = Desk::with(kept, Settings::default(), wrong).await;
    d.env
        .api
        .push(script_turn("tu_1", "Lamps", &thrown("clax.desk.lamps()")));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let value = d.value(&run, 1);
    assert_eq!(value[0], "invalid_result", "{value}");
    assert_eq!(d.journal(&run, 2)[0]["result"]["code"], "invalid_result");
    assert_eq!(d.host.state.lock().unwrap().calls.len(), 1);

    // Away: no `serve` for longer than a registration stays connected.
    d.host.online.store(false, Ordering::SeqCst);
    d.world.clock.advance(HubLimits::default().connected_ms + 1);
    d.env
        .api
        .push(script_turn("tu_1", "Lamps", &thrown("clax.desk.lamps()")));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        d.value(&run, 1),
        json!([
            CODE_UNAVAILABLE,
            "the surface's host is not connected. Nothing happened",
            true
        ])
    );
    assert_eq!(
        d.journal(&run, 2),
        [
            json!({"seq": 0, "op": "desk.lamps", "ok": false, "result": {
            "code": "unavailable", "retryable": true,
            "message": "the surface's host is not connected. Nothing happened"}})
        ]
    );
    assert_eq!(
        d.host.state.lock().unwrap().calls.len(),
        1,
        "nothing was opened for the host"
    );
    // The run's end is recorded all the same, for when its host is
    // back.
    assert_eq!(d.host.waiting(), (0, 2));
}

/// A call of an operation that waits is sent with the long deadline
/// and held by its host; the provider waits for as long as the policy
/// lets a script sit and then answers that the call waits, and the run
/// parks on it: nothing is journaled, no step is committed, and the hub
/// holds the call under its key. (What follows a park is
/// `tests/park.rs`'s.)
async fn a_call_that_waits_is_held_by_its_host_and_journaled_by_no_one(kept: Kept) {
    let d = Desk::new(kept).await;
    d.env.api.push(script_turn(
        "tu_1",
        "Ask",
        &thrown("clax.desk.ask(\"Move the left lamp?\")"),
    ));
    let run = d.run();
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Parked));
    assert_eq!(d.env.run_row(&run).status, RunStatus::Parked);
    assert_eq!(d.journal(&run, 2), Vec::<Value>::new());
    let st = d.host.state.lock().unwrap();
    let [ask] = &st.calls[..] else {
        panic!("{:?}", st.calls);
    };
    assert_eq!(
        (ask.op.as_str(), ask.call, ask.key.as_str()),
        ("ask", wire::CallKind::Effect, format!("{run}:2:0").as_str())
    );
    assert_eq!(ask.args, [json!("Move the left lamp?"), Value::Null]);
    assert_eq!(ask.expires_in_ms, Some(24 * 60 * 60 * 1000));
    drop(st);
    // The provider waited the policy's ten seconds, and no longer.
    assert_eq!(d.world.elapsed(), 10_000);
    assert_eq!(d.host.waiting().0, 1);
    // The run waits on the call until its deadline.
    let parking = d.env.store.run_parking(&run).unwrap().unwrap();
    let on = parking.waiting_on.unwrap();
    assert_eq!(
        (&on["surface"], &on["op"], &on["key"]),
        (
            &json!("desk"),
            &json!("desk.ask"),
            &json!(format!("{run}:2:0"))
        )
    );
    assert_eq!(on["deadline"], json!(d.env.run_row(&run).due_at));
    assert!(on["deadline"].is_string());
}

/// A policy of its own times.
pub(crate) struct Times(pub(crate) Delivery);

impl RunPolicy for Times {
    fn delivery(&self, _run: &RunInfo, _op: &str) -> Delivery {
        self.0
    }
}

/// The policy says how long each call may take, and a registration's
/// settings narrow it: a call's deadline is the lesser of the two, a
/// call that waits with no time to park after is waited for until its
/// deadline, and a deadline's failure is journaled as what is known of
/// the call.
async fn a_calls_deadline_is_the_policys_within_the_registrations_settings(kept: Kept) {
    let settings = Settings {
        call_timeout_s: Some(wire::Bound::At(7)),
        wait_limit_s: Some(wire::Bound::None),
        ..Settings::default()
    };
    let silent: Behave = Box::new(|_| None);
    let d = Desk::with(kept, settings.clone(), silent).await;
    let policy = Delivery {
        call_timeout: Some(Duration::from_secs(20)),
        park_after: None,
        wait_limit: Some(Duration::from_secs(60)),
    };
    assert_eq!(
        narrowed(policy, &settings),
        Delivery {
            call_timeout: Some(Duration::from_secs(7)),
            ..policy
        }
    );
    let widened = Settings {
        call_timeout_s: Some(wire::Bound::At(600)),
        ..Settings::default()
    };
    assert_eq!(narrowed(policy, &widened), policy, "a setting never widens");
    let none = Delivery {
        call_timeout: None,
        park_after: None,
        wait_limit: None,
    };
    assert_eq!(
        narrowed(none, &settings).call_timeout,
        Some(Duration::from_secs(7))
    );
    assert_eq!(Delivery::default(), Delivery::DAEMON);
    assert_eq!(
        DefaultPolicy.delivery(&d.env.info(&d.env.run()), "desk.ask"),
        Delivery::DAEMON
    );

    d.env.api.push(script_turn(
        "tu_1",
        "Both",
        r#"const seen = [];
for (const f of [() => clax.desk.lamps(), () => clax.desk.ask("Well?")]) {
  try { seen.push(await f()); } catch (e) { seen.push(e.code); }
}
return seen;"#,
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let deps = d.deps_under(Arc::new(Times(policy)));
    let out = d.env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    // Each was sent and never answered: a read expires, and an effect's
    // outcome is unknown. Neither parked.
    assert_eq!(d.value(&run, 1), json!(["expired", "unknown_outcome"]));
    let st = d.host.state.lock().unwrap();
    let limits: Vec<Option<u64>> = st.calls.iter().map(|c| c.expires_in_ms).collect();
    assert_eq!(limits, [Some(7_000), Some(60_000)]);
    drop(st);
    assert_eq!(d.world.elapsed(), 67_000);
    let codes: Vec<Value> = d
        .journal(&run, 2)
        .iter()
        .map(|row| row["result"]["code"].clone())
        .collect();
    assert_eq!(codes, ["expired", "unknown_outcome"]);
}

/// A binding that names a registration that was never issued is still
/// bound: its provider takes no call and knows no definition, and
/// answers the request as stored. A registration whose lease has ended
/// keeps the definition a run was bound with, so a host that is away
/// does not keep that run from its first turn. A binding that names no
/// registration is not the resolver's.
async fn a_registration_that_is_gone_takes_no_call_and_its_runs_request_stands(kept: Kept) {
    let d = Desk::new(kept).await;
    let gone = format!("{:032x}", 77);
    let run = d.run_bound(None, Some(&gone), &["desk.lamps"]);
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    let kind = bound.kind_provider();
    let away = kind.unavailable().expect("the registration is gone");
    assert_eq!(away.code, CODE_UNAVAILABLE);
    assert_eq!(
        kind.request(&bound.info).await,
        Ok(json!({"ask": "Turn the lamps down."}))
    );
    // The script's `clax` has what the row keeps for the binding.
    assert_eq!(bound.operations(), ["desk.lamps"]);
    assert_eq!(bound.info.bindings[0].operations, ["desk.lamps"]);
    // A registration whose lease has ended, bound by the hash of its
    // definition, which is kept: the run takes its first turn on the
    // stored request, and its call finds no host.
    let run = d.run_bound(Some(&d.host.hash), Some(&d.host.registration), &TIDYS);
    d.host.online.store(false, Ordering::SeqCst);
    let limits = HubLimits::default();
    d.world
        .clock
        .advance(limits.connected_ms + limits.lease_ms + 1);
    assert!(!d.world.broker.connected(&d.host.registration));
    d.env
        .api
        .push(script_turn("tu_1", "Lamps", &thrown("clax.desk.lamps()")));
    d.env.api.push(finish_turn("done"));
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(
        d.value(&run, 1),
        json!([
            CODE_UNAVAILABLE,
            "the surface's host is gone: its registration has ended. Nothing happened",
            false
        ])
    );
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert!(bound.kind_provider().unavailable().is_none());
    assert_eq!(bound.kind_provider().definition().name(), "desk");
    // No registration named: served by no one, and its kind's request
    // cannot be resolved.
    let unnamed = d.run_bound(None, None, &TIDYS);
    let (row, made) = d.env.run_as_made(&unnamed);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert!(bound.kind_provider().unavailable().is_some());
    assert!(bound.kind_provider().request(&bound.info).await.is_err());
    // A provider registered by name is asked before the resolver.
    let fake: Arc<dyn SurfaceProvider> = Arc::new(clax_bot::surface::FakeSurfaceProvider::new(
        d.env.store.clone(),
    ));
    let by_name = Providers::new()
        .with("desk", fake)
        .with_resolver(d.world.resolver(Arc::new(DefaultPolicy)));
    let (row, made) = d.env.run_as_made(&d.run());
    let bound = by_name.bind(&row, &made).unwrap();
    assert_eq!(bound.kind_provider().definition().name(), "fake");
}

/// A run keeps the definition it was bound with: its binding's hash
/// names it, whatever its host has registered since, and the binding's
/// operations are read from it.
async fn a_run_bound_by_hash_keeps_its_definition_when_its_host_registers_another(kept: Kept) {
    let d = Desk::new(kept).await;
    let bound_with = d.host.hash.clone();
    let run = d.run_bound(Some(&bound_with), Some(&d.host.registration), &TIDYS);
    // The host goes on as its registration with a definition that has
    // no lamps.
    let changed = desk_dts()
        .replace(
            "  /** The lamps on the desk. @read */\n  lamps(): Promise<Lamp[]>;\n",
            "",
        )
        .replace("interface Lamp { id: string; on: boolean }\n", "");
    assert_ne!(changed, desk_dts());
    let request = registering(
        &changed,
        Some(d.host.registration.clone()),
        None,
        Settings::default(),
    );
    let again = (d.world.side.register)("desk", &request, &format!("{:032x}", 50));
    assert_ne!(again.hash, bound_with);
    d.env.api.push(script_turn("tu_1", "Tidy", TIDY));
    d.env.api.push(finish_turn("done"));
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), lamps());
    // A run bound since has the new one.
    let (row, made) = d.env.run_as_made(&d.run());
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    assert!(!bound.operations().contains(&"desk.lamps".to_string()));
}

// ---------------------------------------------------------------------------
// The provider cases
// ---------------------------------------------------------------------------

/// A host of the fake's surface, which answers as the fake does: a
/// counter, a table of values written once for a key, a refusal, and a
/// question it holds.
fn fakes_host() -> (Behave, Arc<Mutex<Vec<Value>>>) {
    let sets = Arc::new(Mutex::new(Vec::new()));
    let kept = sets.clone();
    let mut count = 0u64;
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let behave: Behave = Box::new(move |call| {
        let name = call.args.first().and_then(Value::as_str).unwrap_or("");
        match call.op.as_str() {
            "count" => {
                count += 1;
                result(json!(count))
            }
            "get" => result(values.get(name).map_or(Value::Null, |v| json!(v))),
            "set" => {
                let value = call.args[1].as_str().unwrap_or("");
                values.insert(name.to_string(), value.to_string());
                let mut sets = kept.lock().unwrap();
                sets.push(json!([call.key, call.args]));
                result(json!({"name": name, "writes": sets.len()}))
            }
            "refuse" => error("forbidden", "the host does not keep that"),
            "ask" => None,
            _ => error("broken", "the host is broken"),
        }
    });
    (behave, sets)
}

/// The cases the outside provider is not played through, each for the
/// reason its fixture gives.
const NOT_THE_OUTSIDE_PROVIDERS: [&str; 7] = [
    "a_reads_answer_is_the_providers_and_its_replay_the_journals",
    "a_cancelled_step_keeps_no_write_that_waited_for_its_commit",
    "an_unwritten_failure_leaves_no_row_and_is_made_again",
    "a_waiting_answer_from_an_operation_not_tagged_waits_is_the_providers_error",
    "a_begin_that_fails_leaves_the_step_to_be_begun_again",
    "an_ending_that_failed_is_made_again_in_the_next_attempt",
    "the_hooks_the_provider_was_told_are_what_the_run_loop_owes",
];

/// The outside provider through the provider conformance suite, over
/// the hub in memory and a host of the fake's surface: a counter as the
/// read, a keyed write, a refusal the host answers, the operation the
/// kind does not allow as the one outside the binding, and the question
/// the host holds as the call that waits.
async fn the_outside_provider_passes_the_provider_conformance_suite(kept: Kept) {
    let env = Env::new().await;
    let world = World::new(&env, kept);
    let (behave, sets) = fakes_host();
    let request = registering(FAKE_SURFACE, None, None, Settings::default());
    let host = world.host("fake", &request, behave);
    let stored = BindingAsStored {
        surface: "fake".into(),
        hash: Some(host.hash.clone()),
        registration: Some(host.registration.clone()),
        prompt: None,
        scope: Value::Null,
        granted: Vec::new(),
        operations: None,
    };
    let provider = Arc::new(
        OutsideProvider::new(
            world.broker.clone(),
            Arc::new(DefaultPolicy),
            &stored,
            "fake.job",
        )
        .expect("the binding names a registration"),
    );
    assert!(provider.unavailable().is_none());
    let binding = Binding {
        surface: "fake".into(),
        hash: stored.hash.clone(),
        scope: Value::Null,
        operations: provider
            .definition()
            .operations()
            .map(|o| o.name)
            .filter(|op| provider.standing(op))
            .map(str::to_string)
            .collect(),
    };
    assert!(!binding.has("fake.hidden") && binding.has("fake.ask"));
    let run = |id: String| RunInfo {
        id,
        person: clax_store::roles::OWNER.to_string(),
        model: "claude-sonnet-5-5".to_string(),
        kind: "fake.job".to_string(),
        request: json!({"ask": "Count."}),
        bindings: vec![binding.clone()],
    };
    let state = {
        let sets = sets.clone();
        move || Ok(json!(*sets.lock().unwrap()))
    };
    let applied = {
        let host = host.clone();
        move |key: &str| Ok(u64::from(host.applied(key)))
    };
    let ending_run = run(env.run());
    let fixture = ProviderFixture {
        provider: provider.clone(),
        store: env.store.clone(),
        run: run(env.run()),
        read: OpCall::new("fake.count", json!([])),
        disturb: Maybe::Lacks(
            "a read sent again under its key is the call the broker holds, and is answered what \
             it was answered whether or not its entry comes with it: a replay cannot be told \
             from the call made again (`a_hosts_calls_are_not_sent_again_when_a_crashed_step_is_\
             run_again` holds the replay by what its host was sent)",
        ),
        write: OpCall::new("fake.set", json!(["colour", "blue"])),
        second_write: OpCall::new("fake.set", json!(["size", "large"])),
        state: Box::new(state),
        refused: OpCall::new("fake.refuse", json!(["secret"])),
        outside: OpCall::new("fake.hidden", json!(["x"])),
        cancelled_run: run(env.run()),
        staged: Maybe::Lacks(
            "an outside provider's session has no state: every write is its host's when it is \
             made",
        ),
        ending_run: ending_run.clone(),
        end: End::Finished,
        result: Some(json!({"summary": "counted"})),
        // `ending` writes nothing: an outside provider has no part in
        // the terminal step.
        noticed: Box::new(|| Ok(json!([]))),
        unwritten: Maybe::Lacks(
            "the outside provider answers a call it could not send as a refusal the core \
             journals; it leaves one unwritten only when its broker cannot read a settled call, \
             which the broker in memory always can",
        ),
        waiting: Maybe::Has(Waiting {
            call: OpCall::new("fake.ask", json!(["Which colour?"])),
            answer: json!("blue"),
            applied: Box::new(applied),
        }),
        untagged: Maybe::Lacks(
            "the outside provider answers that a call waits only for an operation its \
             definition tags @waits: nothing a host does makes it answer so for another",
        ),
        fail_once: Maybe::Lacks(
            "the outside provider's `begin` opens a session with no state and its `ending` \
             answers no field: neither reads or writes anything that can fail",
        ),
        lifecycle: Maybe::Lacks(
            "the outside provider keeps no record of its hooks: what it does at a run's end is \
             the end its host is delivered, which this test reads",
        ),
    };
    let results = check_provider(&fixture, &NOT_THE_OUTSIDE_PROVIDERS).await;
    assert_eq!(results.len(), 17);
    for (name, result) in results {
        result.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // Each write reached the host once, however many attempts made it,
    // and the question once.
    let st = host.state.lock().unwrap();
    assert!(st.applied.values().all(|n| *n == 1), "{:?}", st.applied);
    let asked = st.calls.iter().filter(|c| c.op == "ask").count();
    assert_eq!(asked, 1);
    assert_eq!(sets.lock().unwrap().len(), 3);
    assert!(
        !st.calls.iter().any(|c| c.op == "hidden"),
        "the operation outside the binding was never sent"
    );
    drop(st);
    // Both runs the cases ended reach the host, each once though the
    // second was told twice, the finished one with its result.
    host.round();
    let st = host.state.lock().unwrap();
    // (Ends kept at one instant are delivered in an order of the
    // store's own.)
    let mut ended: Vec<(&str, &wire::End, &Option<Value>)> = st
        .ended
        .iter()
        .map(|e| (e.run.as_str(), &e.end, &e.result))
        .collect();
    ended.sort_by_key(|(_, end, _)| matches!(end, wire::End::Finished {}));
    assert_eq!(
        ended,
        [
            (
                fixture.cancelled_run.id.as_str(),
                &wire::End::Stopped {
                    reason: "cancelled".into()
                },
                &None
            ),
            (
                ending_run.id.as_str(),
                &wire::End::Finished {},
                &Some(json!({"summary": "counted"}))
            ),
        ]
    );
}

/// A call that waits is answered a day later, by a host that brings the
/// answer when no run is waiting for it: the step run again has the
/// answer, from the journal where the store wrote it when it arrived,
/// and from the broker, which kept it, where nothing did. The host was
/// asked once.
async fn a_call_that_waited_is_answered_when_its_step_is_run_again(kept: Kept) {
    let d = Desk::new(kept).await;
    let run = d.run();
    let (row, made) = d.env.run_as_made(&run);
    let bound = d.deps().providers.bind(&row, &made).unwrap();
    let info = &bound.info;
    let provider = bound.kind_provider();
    let binding = &info.bindings[0];
    let driver = DriverId::mint();
    d.env.store.claim_run(&run, &driver, 60).unwrap().unwrap();
    let cancel = clax_bot_sandbox::CancelToken::new();
    let raw = json!(["Move the left lamp?"]);
    // One attempt of the script step, and its call of `desk.ask`.
    let ask = || async {
        let step = d
            .env
            .store
            .begin_step(&run, &driver, 1, StepKind::Script)
            .unwrap();
        let claim = Claim::new(&d.env.store, &driver, &run);
        let cx = StepCx::new(info, binding, 1, step.attempt, &[], claim, &cancel);
        let session = provider.begin(cx).await.unwrap();
        let journal = d.env.store.bot_calls(&run, 1).unwrap();
        let place = cx.place(0).unwrap();
        let held = [Bound {
            binding,
            provider,
            session: &*session,
        }];
        let entry = journal.iter().find(|c| c.seq() == 0);
        let answer = dispatch(&held, &claim, &place, "desk.ask", &raw, entry).await;
        (answer, entry.is_some())
    };
    let (first, _) = ask().await;
    assert!(first.is_waiting());
    assert_eq!(d.journal(&run, 1), Vec::<Value>::new());
    assert_eq!(d.host.waiting().0, 1);
    // A day later, less a minute, the host is back, goes on as its
    // registration, and brings the answer.
    d.world.clock.advance(24 * 60 * 60 * 1000 - 70_000);
    let back = registering(
        &desk_dts(),
        Some(d.host.registration.clone()),
        None,
        Settings::default(),
    );
    (d.world.side.register)("desk", &back, &format!("{:032x}", 60));
    let id = d.host.state.lock().unwrap().calls[0].id.clone();
    d.host.answer(&id, wire::Outcome::Result(json!("yes")));
    assert_eq!(d.host.waiting().0, 0);
    let journaled = d.journal(&run, 1);
    match kept {
        // Whoever brought the answer journaled it, with no claim held.
        Kept::InTheStore => assert_eq!(
            journaled,
            [json!({"seq": 0, "op": "desk.ask", "ok": true, "result": "yes"})]
        ),
        Kept::InMemory => assert_eq!(journaled, Vec::<Value>::new()),
    }
    let (again, replayed) = ask().await;
    assert_eq!(replayed, kept == Kept::InTheStore);
    assert_eq!(again.result(), &Ok(json!("yes")));
    assert_eq!(
        d.journal(&run, 1),
        [json!({"seq": 0, "op": "desk.ask", "ok": true, "result": "yes"})]
    );
    assert_eq!(d.host.applied(&format!("{run}:1:0")), 1);
    assert_eq!(d.host.state.lock().unwrap().calls.len(), 1);
}

/// The fixture's surface with no lamps: another definition of `desk`.
fn desk_without_lamps() -> String {
    let changed = desk_dts()
        .replace(
            "  /** The lamps on the desk. @read */\n  lamps(): Promise<Lamp[]>;\n",
            "",
        )
        .replace("interface Lamp { id: string; on: boolean }\n", "");
    assert_ne!(changed, desk_dts());
    changed
}

/// The system prompt the first turn of `run` is sent, driven to its
/// end over `deps` with one turn that finishes.
async fn system_sent(env: &Env, deps: &Deps, run: &str) -> String {
    system_sent_by(env, deps, run, &DriverId::mint()).await
}

/// [`system_sent`], driven by `driver`.
async fn system_sent_by(env: &Env, deps: &Deps, run: &str, driver: &DriverId) -> String {
    let before = env.api.requests().len();
    env.api.push(finish_turn("done"));
    let out = env.drive(deps, run, driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    env.api.requests()[before].body["system"][0]["text"]
        .as_str()
        .expect("the turn's system prompt")
        .to_string()
}

/// The system prompt a run of `desk.tidy` is sent when its host only
/// ever registered `definition` with `prompt`: what a run started with
/// the two is to be sent, to the byte.
async fn system_of_a_host_that_registered_only(
    kept: Kept,
    definition: &str,
    prompt: Option<&str>,
) -> String {
    let env = Env::new().await;
    let world = World::new(&env, kept);
    let request = registering(definition, None, prompt, Settings::default());
    let host = world.host("desk", &request, desk_host());
    let d = Desk { env, world, host };
    let hash = prompt.map(|p| clax_core::audit::sha256_hex(p.as_bytes()));
    let run = d.run_started(
        Some(&d.host.hash),
        Some(&d.host.registration),
        hash.as_deref(),
        &TIDYS,
    );
    system_sent(&d.env, &d.deps(), &run).await
}

/// A run is sent the prompt file and the definition it was started
/// with, by the hashes its binding names: a host that registers others
/// before the run's first turn changes them for the runs started
/// afterwards alone; a hub that does not hold the registration (a
/// daemon that started since) still sends the run its own; a run
/// started with no prompt file is sent the standard prompt; a prompt
/// file that cannot be read now fails the tick and leaves the run to be
/// ticked again; and a prompt file that is no longer kept ends the run
/// `failed` `prompt_gone`, with no request sent.
async fn a_run_is_sent_the_prompt_file_and_definition_it_was_started_with(kept: Kept) {
    let sha = |text: &str| clax_core::audit::sha256_hex(text.as_bytes());
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let first = fixture["files"]["desk.prompt.md"].as_str().unwrap();
    let second = first.replace("You look after a desk", "You keep another desk");
    assert_ne!(second, first);
    let (first_def, second_def) = (desk_dts(), desk_without_lamps());
    // What each pair is sent as, and the standard prompt, each from a
    // host that registered nothing else.
    let first_system = system_of_a_host_that_registered_only(kept, &first_def, Some(first)).await;
    let second_system =
        system_of_a_host_that_registered_only(kept, &second_def, Some(&second)).await;
    let standard = system_of_a_host_that_registered_only(kept, &first_def, None).await;
    assert!(
        first_system.starts_with("You look after a desk"),
        "{first_system}"
    );
    assert!(
        first_system.contains("lamps(): Promise<desk.Lamp[]>"),
        "{first_system}"
    );
    assert!(
        second_system.starts_with("You keep another desk"),
        "{second_system}"
    );
    assert!(!second_system.contains("lamps()"), "{second_system}");
    assert!(!standard.starts_with("You look after a desk"), "{standard}");

    let env = Env::new().await;
    let world = World::new(&env, kept);
    let request = registering(&first_def, None, Some(first), Settings::default());
    let host = world.host("desk", &request, desk_host());
    let d = Desk { env, world, host };
    let registration = d.host.registration.clone();
    let started = |hash: &str, prompt: Option<&str>| {
        d.run_started(Some(hash), Some(&registration), prompt, &TIDYS)
    };
    // Two runs started with the first pair, neither driven yet.
    let first_hash = d.host.hash.clone();
    let early = started(&first_hash, Some(&sha(first)));
    let over_a_restart = started(&first_hash, Some(&sha(first)));
    // The host goes on as its registration with another definition and
    // another prompt file, and a run is started with those.
    let request = registering(
        &second_def,
        Some(registration.clone()),
        Some(&second),
        Settings::default(),
    );
    let again = (d.world.side.register)("desk", &request, &format!("{:032x}", 50));
    assert_ne!(again.hash, first_hash);
    let late = started(&again.hash, Some(&sha(&second)));
    // Each is sent what it was started with, to the byte.
    assert_eq!(system_sent(&d.env, &d.deps(), &early).await, first_system);
    assert_eq!(system_sent(&d.env, &d.deps(), &late).await, second_system);
    // A run started with no prompt file is sent the standard prompt,
    // whatever its registration has.
    let plain = started(&first_hash, None);
    assert_eq!(system_sent(&d.env, &d.deps(), &plain).await, standard);
    // A daemon that started since: a new hub over the same store, which
    // does not hold the registration until its host registers again.
    if kept == Kept::InTheStore {
        let restarted = World::new(&d.env, kept);
        assert!(!restarted.broker.connected(&registration));
        let mut deps = d.env.deps();
        deps.providers
            .set_resolver(restarted.resolver(Arc::new(DefaultPolicy)));
        assert_eq!(
            system_sent(&d.env, &deps, &over_a_restart).await,
            first_system
        );
    }
    // A prompt file that cannot be read now: the tick fails, nothing is
    // sent and the run has not ended; read again, it is the run's own.
    let unread = started(&first_hash, Some(&sha(first)));
    let requests = d.env.api.requests().len();
    d.world.broker.prompts_unread.store(true, Ordering::SeqCst);
    let driver = DriverId::mint();
    let failed = d.env.try_tick(&d.deps(), &unread, &driver, None).await;
    match failed {
        Err(clax_core::CoreError::Invalid { code, message }) => {
            assert_eq!(code, "prompt_unread");
            assert!(message.contains("disk I/O error"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(d.env.api.requests().len(), requests, "no turn was sent");
    assert_eq!(d.env.run_row(&unread).ended_at, None);
    d.world.broker.prompts_unread.store(false, Ordering::SeqCst);
    assert_eq!(
        system_sent_by(&d.env, &d.deps(), &unread, &driver).await,
        first_system
    );
    // A prompt file that is not kept: the run ends, and is sent no
    // other prompt in its place.
    let lost = started(&first_hash, Some(&sha("a prompt file nothing kept")));
    let requests = d.env.api.requests().len();
    let out = d.env.drive(&d.deps(), &lost, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Failed));
    assert_eq!(d.env.run_row(&lost).reason.as_deref(), Some("prompt_gone"));
    assert_eq!(d.env.api.requests().len(), requests, "no turn was sent");
}

over_both_stores! {
    /// A run calls its host's operations and is told of at its end.
    answered => a_hosts_surface_answers_a_runs_calls_and_is_told_its_end;
    /// A crashed step run again sends nothing.
    replayed => a_hosts_calls_are_not_sent_again_when_a_crashed_step_is_run_again;
    away_and_wrong => a_host_that_is_away_is_unavailable_and_a_wrong_answer_is_an_invalid_result;
    waiting => a_call_that_waits_is_held_by_its_host_and_journaled_by_no_one;
    deadlines => a_calls_deadline_is_the_policys_within_the_registrations_settings;
    gone => a_registration_that_is_gone_takes_no_call_and_its_runs_request_stands;
    by_hash => a_run_bound_by_hash_keeps_its_definition_when_its_host_registers_another;
    conformance => the_outside_provider_passes_the_provider_conformance_suite;
    answered_later => a_call_that_waited_is_answered_when_its_step_is_run_again;
    prompt => a_run_is_sent_the_prompt_file_and_definition_it_was_started_with;
}

// ---------------------------------------------------------------------------
// The sources
// ---------------------------------------------------------------------------

/// The crates the run loop is made of: this one and those it depends
/// on. (The daemon's crates are above it; their scan is the daemon's
/// end-to-end test's.)
const CRATES: [&str; 7] = [
    "clax-bot",
    "clax-bot-sandbox",
    "clax-core",
    "clax-rt",
    "clax-sample",
    "clax-store",
    "clax-surface",
];

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Whether `text` holds `word` as a word of its own.
fn names(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

/// The files and directories under `files` that are modules of the
/// tests alone: each `mod <name>;` a file declares under
/// `#[cfg(test)]`, as `<name>.rs` and `<name>/` beside the file (for
/// `lib.rs`, `main.rs` and `mod.rs`) or under the directory of its own
/// name.
fn test_modules(files: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
    {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let dir = path.parent().unwrap();
        let base = if ["lib", "main", "mod"].contains(&stem) {
            dir.to_path_buf()
        } else {
            dir.join(stem)
        };
        let mut lines = text.lines().map(str::trim).peekable();
        while let Some(line) = lines.next() {
            if line != "#[cfg(test)]" {
                continue;
            }
            let declared = lines.peek().and_then(|next| {
                next.rsplit_once("mod ")
                    .and_then(|(_, name)| name.strip_suffix(';'))
            });
            if let Some(name) = declared {
                out.push(base.join(format!("{name}.rs")));
                out.push(base.join(name));
            }
        }
    }
    out
}

/// No source of the run loop names the fixture's surface: a file under
/// `src` of this crate or of a crate it depends on, but for its tests
/// (a module declared under `#[cfg(test)]`, a file under a `tests`
/// directory, what follows `#[cfg(test)]` in any file, and a `.golden`
/// file a test pins) and the records kept beside the code. The surface
/// reaches a run from its host's registration and from nowhere else.
#[test]
fn no_source_of_the_run_loop_names_the_fixtures_surface() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut read = 0;
    for name in CRATES {
        let mut files = Vec::new();
        sources(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name} has no sources here");
        let tests = test_modules(&files);
        for path in files {
            let shown = path.display().to_string();
            let test = path.components().any(|c| c.as_os_str() == "tests")
                || tests.iter().any(|t| path.starts_with(t));
            if test || shown.ends_with(".qual") || shown.ends_with(".golden") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            read += 1;
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            assert!(
                !names(&code.to_lowercase(), "desk"),
                "{shown} names the fixture's surface"
            );
        }
    }
    assert!(read > 100, "{read} sources were read");
    assert!(names("a desk.", "desk") && !names("desktop", "desk") && !names("_desk", "desk"));
}
