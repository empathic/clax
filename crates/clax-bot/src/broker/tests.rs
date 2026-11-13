//! The broker over the hub, with the fake surface's file registered as
//! a host's and the host played by hand through [`HubBroker::with`].

use super::*;
use crate::bot_surfaces::HubLimits;
use crate::surface::FAKE_SURFACE;
use clax_store::Store;
use clax_surface::Reserved;
use clax_surface::wire::{self, Answer, Delivery, Notation, RegisterRequest};
use serde_json::json;

const RUN: &str = "run-1";

/// The first registration's ID.
fn rid() -> String {
    format!("{:032x}", 1)
}

fn request(definition: &str, registration: Option<String>) -> RegisterRequest {
    RegisterRequest {
        clax_surface: wire::VERSION,
        notation: Notation::Dts,
        definition: definition.to_string(),
        registration,
        prompt: None,
        settings: Settings::default(),
    }
}

/// A [`BrokerStore`] that keeps what it was told of each call it
/// opened.
#[derive(Default)]
struct Told {
    memory: MemoryPending,
    opened: Vec<(String, i64, i64, i64, String, String, bool)>,
}

impl Pending for Told {
    type Error = std::convert::Infallible;
    fn issue_registration(
        &mut self,
        id: &str,
        name: &str,
        hash: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        self.memory.issue_registration(id, name, hash, now)
    }
    fn registration_name(&mut self, id: &str) -> Result<Option<String>, Self::Error> {
        self.memory.registration_name(id)
    }
    fn keep_definition(&mut self, hash: &str, text: &str, now: Millis) -> Result<(), Self::Error> {
        self.memory.keep_definition(hash, text, now)
    }
    fn definition_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        self.memory.definition_text(hash)
    }
    fn keep_prompt(&mut self, hash: &str, text: &str, now: Millis) -> Result<(), Self::Error> {
        self.memory.keep_prompt(hash, text, now)
    }
    fn prompt_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        self.memory.prompt_text(hash)
    }
    fn open_call(
        &mut self,
        _call: NewCall,
        _limit: usize,
        _now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        panic!("a broker opens a call with its place");
    }
    fn pending_for(&mut self, registration: &str) -> Result<PendingSummary, Self::Error> {
        self.memory.pending_for(registration)
    }
    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error> {
        self.memory.calls_for(registration, ids)
    }
    fn sent_calls(&mut self, ids: &[String]) -> Result<(), Self::Error> {
        self.memory.sent_calls(ids)
    }
    fn accept_call(&mut self, reg: &str, id: &str, now: Millis) -> Result<bool, Self::Error> {
        self.memory.accept_call(reg, id, now)
    }
    fn answer_call(
        &mut self,
        reg: &str,
        id: &str,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        self.memory.answer_call(reg, id, outcome, now)
    }
    fn expire_calls(
        &mut self,
        now: Millis,
        failure: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error> {
        self.memory.expire_calls(now, failure)
    }
    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
        self.memory.next_deadline()
    }
    fn record_end(
        &mut self,
        end: NewEnd,
        limit: usize,
        now: Millis,
    ) -> Result<RecordedEnd, Self::Error> {
        self.memory.record_end(end, limit, now)
    }
    fn ends_for(&mut self, reg: &str, ids: &[String]) -> Result<Vec<PendingEnd>, Self::Error> {
        self.memory.ends_for(reg, ids)
    }
    fn acknowledge_end(&mut self, reg: &str, id: &str, now: Millis) -> Result<bool, Self::Error> {
        self.memory.acknowledge_end(reg, id, now)
    }
    fn expire_ends(&mut self, before: Millis) -> Result<usize, Self::Error> {
        self.memory.expire_ends(before)
    }
}

impl BrokerStore for Told {
    fn open_placed(
        &mut self,
        at: &Placed<'_>,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        self.opened.push((
            at.run.to_string(),
            at.place.step,
            at.place.attempt,
            at.place.seq,
            at.driver.as_str().to_string(),
            at.args_hash.clone(),
            at.waits,
        ));
        assert_eq!(at.place.key, call.key);
        self.memory.open_call(call, limit, now)
    }
    fn settled(&mut self, id: &str) -> Result<Option<Result<Value, CallFailure>>, Self::Error> {
        self.memory.settled(id)
    }
}

/// A broker with the fake surface registered at 0, a store for claims,
/// and the driver that holds them.
struct World {
    broker: Arc<HubBroker<Told>>,
    clock: Arc<SkipClock>,
    store: Store,
    driver: DriverId,
    hash: String,
    _dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let clock = Arc::new(SkipClock::at(0));
        let hub = Hub::new(
            HubLimits::default(),
            Reserved::of(&[crate::api::definition()]),
            Box::new(|_| {}),
        );
        let broker = Arc::new(HubBroker::new(hub, Told::default(), clock.clone()));
        let registered = broker
            .with(|hub, store, now| {
                hub.register(store, "fake", &request(FAKE_SURFACE, None), &rid(), now)
            })
            .expect("the fake's file registers as a host's");
        assert_eq!(registered.registration, rid());
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        World {
            broker,
            clock,
            store,
            driver: DriverId::mint(),
            hash: registered.hash,
            _dir: dir,
        }
    }

    fn send_at(
        &self,
        seq: i64,
        op: &str,
        args: Value,
        deadline: Option<u64>,
    ) -> Result<String, CallFailure> {
        let place = CallPlace::new(RUN, 2, 3, seq);
        let waits = op == "fake.ask";
        self.broker.send(
            &Claim::new(&self.store, &self.driver, RUN),
            &Sending {
                registration: &rid(),
                definition: &self.hash,
                run: RUN,
                op,
                args: args.as_array().unwrap(),
                raw: &args,
                place: &place,
                waits,
                deadline: deadline.map(Millis),
            },
        )
    }

    /// One `serve` of the host's, bringing `answers`.
    fn serve(&self, answers: Vec<Answer>) -> Vec<Delivery> {
        self.broker
            .with(|hub, store, now| hub.serve(store, "fake", &rid(), answers, now))
            .expect("the registration serves")
    }

    fn calls(&self, answers: Vec<Answer>) -> Vec<wire::Call> {
        self.serve(answers)
            .into_iter()
            .filter_map(|d| match d {
                Delivery::Call(c) => Some(c),
                _ => None,
            })
            .collect()
    }
}

fn result(id: &str, value: Value) -> Answer {
    Answer {
        id: id.to_string(),
        outcome: wire::Outcome::Result(value),
    }
}

/// A call goes to its host with the key of its place, its run, its
/// operation below the surface's name, its arguments, what it is to
/// its surface and its deadline; the store is told the claim's driver
/// and the place; and its outcome is the host's answer, checked.
#[tokio::test]
async fn a_call_is_sent_with_its_key_and_deadline_and_its_checked_answer_comes_back() {
    let w = World::new();
    w.clock.set(1_000);
    let id = w
        .send_at(4, "fake.set", json!(["colour", "blue"]), Some(31_000))
        .unwrap();
    let args_hash = args_hash(&json!(["colour", "blue"]));
    w.broker.with(|_, store, _| {
        assert_eq!(
            store.opened,
            [(
                RUN.to_string(),
                2,
                3,
                4,
                w.driver.as_str().to_string(),
                args_hash.clone(),
                false
            )]
        );
    });
    // Not answered: nothing yet, and the clock has moved to the instant
    // asked for.
    assert_eq!(w.broker.outcome(&id, Millis(2_000)).await, Outcome::NotYet);
    assert_eq!(w.broker.now(), Millis(2_000));
    let sent = w.calls(Vec::new());
    assert_eq!(sent.len(), 1);
    let call = &sent[0];
    assert_eq!(call.id, id);
    assert_eq!(call.key, format!("{RUN}:2:4"));
    assert_eq!(call.run, RUN);
    assert_eq!(call.op, "set");
    assert_eq!(call.args, [json!("colour"), json!("blue")]);
    assert_eq!(call.call, wire::CallKind::Write);
    assert_eq!(call.expires_in_ms, Some(29_000));
    // A result of another type than the operation declares is
    // `invalid_result`; the first answer stands.
    let id2 = w.send_at(5, "fake.count", json!([]), None).unwrap();
    w.serve(vec![
        result(&id, json!({"name": "colour", "writes": 1})),
        result(&id2, json!("seven")),
    ]);
    assert_eq!(
        w.broker.outcome(&id, Millis(2_000)).await,
        Outcome::Settled(Ok(json!({"name": "colour", "writes": 1})))
    );
    let Outcome::Settled(Err(wrong)) = w.broker.outcome(&id2, Millis(2_000)).await else {
        panic!("the wrong-typed answer settled the call as a value");
    };
    assert_eq!(wrong.code, "invalid_result");
    // A settled outcome is read again, and the call sent again with
    // its key is the same call: its host is sent nothing more.
    assert_eq!(
        w.send_at(4, "fake.set", json!(["colour", "blue"]), Some(40_000))
            .unwrap(),
        id
    );
    assert!(w.calls(Vec::new()).is_empty());
    assert_eq!(
        w.broker.outcome(&id, Millis(2_000)).await,
        Outcome::Settled(Ok(json!({"name": "colour", "writes": 1})))
    );
}

/// A call not answered by its deadline is settled with the deadline's
/// failure: `unavailable` for one no `serve` was answered with, and for
/// one that was, `expired` for a read and `unknown_outcome` for a
/// write.
#[tokio::test]
async fn a_call_past_its_deadline_is_settled_with_what_is_known_of_it() {
    let w = World::new();
    let unsent = w.send_at(0, "fake.count", json!([]), Some(5_000)).unwrap();
    assert_eq!(
        w.broker.outcome(&unsent, Millis(4_999)).await,
        Outcome::NotYet
    );
    let Outcome::Settled(Err(e)) = w.broker.outcome(&unsent, Millis(5_000)).await else {
        panic!("the deadline did not settle the call");
    };
    assert_eq!((e.code.as_str(), e.retryable), ("unavailable", true));
    let read = w.send_at(1, "fake.count", json!([]), Some(9_000)).unwrap();
    let write = w
        .send_at(2, "fake.set", json!(["a", "b"]), Some(9_000))
        .unwrap();
    assert_eq!(w.calls(Vec::new()).len(), 2);
    for (id, code) in [(read, "expired"), (write, "unknown_outcome")] {
        let Outcome::Settled(Err(e)) = w.broker.outcome(&id, Millis(9_000)).await else {
            panic!("the deadline did not settle call {id}");
        };
        assert_eq!((e.code.as_str(), e.retryable), (code, false));
    }
    // With no deadline given the call has the hub's own.
    let open = w.send_at(3, "fake.ask", json!(["q"]), None).unwrap();
    let day = HubLimits::default().call_ms;
    let sent = w.calls(Vec::new());
    assert_eq!(sent[0].expires_in_ms, Some(day));
    assert_eq!(
        w.broker.outcome(&open, Millis(10_000)).await,
        Outcome::NotYet
    );
    w.broker
        .with(|_, store, _| assert!(store.opened[3].6, "the call that waits is marked"));
}

/// A registration that is not connected, or not held, takes no call:
/// the script is thrown `unavailable`, and nothing is opened.
#[tokio::test]
async fn a_registration_that_is_away_takes_no_call() {
    let w = World::new();
    assert!(w.broker.connected(&rid()));
    w.clock.set(HubLimits::default().connected_ms + 1);
    assert!(!w.broker.connected(&rid()));
    let e = w.send_at(0, "fake.count", json!([]), None).unwrap_err();
    assert_eq!((e.code.as_str(), e.retryable), ("unavailable", true));
    assert_eq!(
        e.message,
        "the surface's host is not connected. Nothing happened"
    );
    // Its lease over, the registration is not held: no call, and no
    // definition without its hash.
    w.clock.advance(HubLimits::default().lease_ms + 1);
    let e = w.send_at(0, "fake.count", json!([]), None).unwrap_err();
    assert_eq!((e.code.as_str(), e.retryable), ("unavailable", false));
    assert!(w.broker.registration(&rid(), None).is_none());
    let by_hash = w.broker.registration(&rid(), Some(&w.hash)).unwrap();
    assert!(!by_hash.held);
    assert_eq!(by_hash.name, "fake");
    assert!(
        w.broker
            .registration(&format!("{:032x}", 9), Some(&w.hash))
            .is_none()
    );
    w.broker
        .with(|_, store, _| assert!(store.opened.is_empty()));
}

/// A registration is looked up with the definition kept under a hash:
/// a run bound before its host registered another definition keeps the
/// one it was bound with, and its call is read and checked by it.
#[tokio::test]
async fn a_call_is_made_under_the_definition_its_run_was_bound_with() {
    let w = World::new();
    let first = w.broker.registration(&rid(), None).unwrap();
    assert_eq!((first.hash.as_str(), first.held), (w.hash.as_str(), true));
    assert!(first.withheld.is_empty());
    // The host registers again, as the same registration, without
    // `fake.count` and with `fake.get` answering a number.
    let changed = FAKE_SURFACE
        .replace(
            "  /** How many times this was read, this time included. @read */\n  count(): Promise<Int>;\n",
            "",
        )
        .replace("get(name: string): Promise<string | null>", "get(name: string): Promise<Int>")
        .replace("@allow fake.count ", "@allow ");
    let again = w
        .broker
        .with(|hub, store, now| {
            hub.register(
                store,
                "fake",
                &request(&changed, Some(rid())),
                &format!("{:032x}", 2),
                now,
            )
        })
        .expect("the changed file registers");
    assert_ne!(again.hash, w.hash);
    let now = w.broker.registration(&rid(), None).unwrap();
    assert_eq!(now.hash, again.hash);
    assert!(now.definition.operation("fake.count").is_none());
    let bound = w.broker.registration(&rid(), Some(&w.hash)).unwrap();
    assert!(bound.definition.operation("fake.count").is_some() && bound.held);
    assert!(
        w.broker
            .registration(&rid(), Some("no such hash"))
            .is_none()
    );
    // Under the hash the run was bound with: the operation is there,
    // and a text is `fake.get`'s result.
    let count = w.send_at(0, "fake.count", json!([]), None).unwrap();
    let get = w.send_at(1, "fake.get", json!(["colour"]), None).unwrap();
    w.serve(vec![result(&count, json!(3)), result(&get, json!("blue"))]);
    for (id, value) in [(count, json!(3)), (get, json!("blue"))] {
        assert_eq!(
            w.broker.outcome(&id, Millis(0)).await,
            Outcome::Settled(Ok(value))
        );
    }
    // Under the registration's definition of now, neither holds.
    let place = CallPlace::new(RUN, 2, 3, 7);
    let args = json!([]);
    let e = w
        .broker
        .send(
            &Claim::new(&w.store, &w.driver, RUN),
            &Sending {
                registration: &rid(),
                definition: &again.hash,
                run: RUN,
                op: "fake.count",
                args: &[],
                raw: &args,
                place: &place,
                waits: false,
                deadline: None,
            },
        )
        .unwrap_err();
    assert_eq!(e.code, "unknown_op");
}

/// A run's end is recorded once for its registration and delivered
/// with each `serve` until an answer names it.
#[tokio::test]
async fn a_runs_end_is_delivered_until_it_is_acknowledged() {
    let w = World::new();
    let end = || End::Finished {};
    let id = w
        .broker
        .ended(&rid(), RUN, end(), Some(json!({"summary": "done"})))
        .unwrap();
    assert_eq!(w.broker.ended(&rid(), RUN, end(), None).unwrap(), id);
    for _ in 0..2 {
        let sent = w.serve(Vec::new());
        let [Delivery::Ended(ended)] = &sent[..] else {
            panic!("{sent:?}");
        };
        assert_eq!((ended.id.as_str(), ended.run.as_str()), (id.as_str(), RUN));
        assert_eq!(ended.end, end());
        assert_eq!(ended.result, Some(json!({"summary": "done"})));
    }
    assert!(w.serve(vec![result(&id, Value::Null)]).is_empty());
    assert!(
        w.broker
            .ended(&format!("{:032x}", 9), RUN, end(), None)
            .is_err()
    );
}

/// A clock no wait moves.
struct Still;

#[async_trait::async_trait]
impl BrokerClock for Still {
    fn now(&self) -> Millis {
        Millis(0)
    }
    async fn until(&self, _at: Millis) {
        std::future::pending::<()>().await;
    }
}

/// Whoever waits for a call's outcome is woken by the `serve` that
/// brings its answer, with no clock moving.
#[tokio::test]
async fn an_answer_reaches_whoever_waits_for_it_without_the_clock_moving() {
    let hub = Hub::new(
        HubLimits::default(),
        Reserved::of(&[crate::api::definition()]),
        Box::new(|_| {}),
    );
    let broker = HubBroker::new(hub, MemoryPending::new(), Arc::new(Still));
    broker
        .with(|hub, store, now| {
            hub.register(store, "fake", &request(FAKE_SURFACE, None), &rid(), now)
        })
        .unwrap();
    let hash = broker.registration(&rid(), None).unwrap().hash;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
    let driver = DriverId::mint();
    let place = CallPlace::new(RUN, 1, 1, 0);
    let args = json!([]);
    let id = broker
        .send(
            &Claim::new(&store, &driver, RUN),
            &Sending {
                registration: &rid(),
                definition: &hash,
                run: RUN,
                op: "fake.count",
                args: &[],
                raw: &args,
                place: &place,
                waits: false,
                deadline: None,
            },
        )
        .unwrap();
    let host = async {
        broker
            .with(|hub, store, now| {
                hub.serve(store, "fake", &rid(), vec![result(&id, json!(7))], now)
            })
            .unwrap();
    };
    let (outcome, ()) = tokio::join!(broker.outcome(&id, Millis(60_000)), host);
    assert_eq!(outcome, Outcome::Settled(Ok(json!(7))));
}
