//! The hub's tests: the contract fixture's script replayed against it
//! (`docs/claxbot-surfaces/contract.json`), its limits and its clock
//! one rule at a time, a store that fails, and generated sequences of
//! everything hosts, runs and the clock can do, held to a model.

use super::*;
use clax_surface::wire::{CallError, ServeRequest};
use serde_json::json;
use std::sync::{Arc, Mutex};

const FIXTURE: &str = include_str!("../../../../docs/claxbot-surfaces/contract.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the fixture is JSON")
}

fn desk_dts() -> String {
    fixture()["files"]["desk.d.ts"]
        .as_str()
        .expect("the fixture has desk.d.ts")
        .to_string()
}

/// The fixture's surface under another name.
fn shelf_dts() -> String {
    desk_dts()
        .replace("declare const desk", "declare const shelf")
        .replace("@allow desk.*", "@allow shelf.*")
}

/// The `n`th registration ID of a test: the form of one, and nothing
/// drawn.
fn rid(n: u64) -> String {
    format!("{n:032x}")
}

/// A hub with these limits, and the registrations its hook was called
/// with.
fn hub(limits: HubLimits) -> (Hub, Arc<Mutex<Vec<String>>>) {
    let woken = Arc::new(Mutex::new(Vec::new()));
    let hook = woken.clone();
    let hub = Hub::new(
        limits,
        Reserved::of(&[]),
        Box::new(move |id| hook.lock().unwrap().push(id.to_string())),
    );
    (hub, woken)
}

fn request(definition: &str) -> RegisterRequest {
    RegisterRequest {
        clax_surface: wire::VERSION,
        notation: Notation::Dts,
        definition: definition.to_string(),
        registration: None,
        prompt: None,
        settings: Settings::default(),
    }
}

/// `request`, to go on as registration `n`.
fn again(n: u64, definition: &str) -> RegisterRequest {
    RegisterRequest {
        registration: Some(rid(n)),
        ..request(definition)
    }
}

fn call(op: &str, args: Value, key: &str, deadline: Option<u64>) -> Enqueue {
    Enqueue {
        run: "run-1".to_string(),
        op: op.to_string(),
        args: args.as_array().expect("arguments are an array").clone(),
        key: key.to_string(),
        deadline: deadline.map(Millis),
    }
}

fn result(id: &str, value: Value) -> Answer {
    Answer {
        id: id.to_string(),
        outcome: Outcome::Result(value),
    }
}

fn error(id: &str, code: &str, message: &str) -> Answer {
    Answer {
        id: id.to_string(),
        outcome: Outcome::Error(CallError {
            code: code.into(),
            message: message.into(),
        }),
    }
}

fn ids(deliveries: &[Delivery]) -> Vec<&str> {
    deliveries.iter().filter_map(Delivery::id).collect()
}

// ---------------------------------------------------------------------------
// The contract fixture's script
// ---------------------------------------------------------------------------

/// The fixture's script, replayed: what `$` names stand for, and the
/// hub and store it runs against.
struct Replay {
    hub: Hub,
    store: MemoryPending,
    files: Value,
    names: BTreeMap<String, Value>,
    registered: u64,
}

impl Replay {
    /// `v` as a request: its files and names filled in.
    fn filled(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => {
                if let Some(file) = s.strip_prefix("$file:") {
                    self.files[file].clone()
                } else if s.starts_with('$') {
                    self.names
                        .get(s)
                        .unwrap_or_else(|| panic!("{s} is used before the daemon gave it"))
                        .clone()
                } else {
                    v.clone()
                }
            }
            Value::Array(items) => items.iter().map(|i| self.filled(i)).collect(),
            Value::Object(o) => o
                .iter()
                .map(|(k, i)| (k.clone(), self.filled(i)))
                .collect::<serde_json::Map<_, _>>()
                .into(),
            _ => v.clone(),
        }
    }

    /// Holds `got` to `want`: equal, but that `$_` is anything and a
    /// `$name` is whatever it first was, and no other name's value.
    fn held(&mut self, want: &Value, got: &Value, step: &Value) {
        match (want, got) {
            (Value::String(s), _) if s == "$_" => {}
            (Value::String(s), _) if s.starts_with('$') => match self.names.get(s) {
                Some(known) => assert_eq!(known, got, "{s}\n{step}"),
                None => {
                    assert!(
                        !self.names.values().any(|v| v == got),
                        "{s} is {got}, which another name is\n{step}"
                    );
                    self.names.insert(s.clone(), got.clone());
                }
            },
            (Value::Array(want), Value::Array(got)) => {
                assert_eq!(want.len(), got.len(), "{want:?}\n{got:?}\n{step}");
                for (w, g) in want.iter().zip(got) {
                    self.held(w, g, step);
                }
            }
            (Value::Object(want), Value::Object(got_o)) => {
                assert_eq!(
                    want.keys().collect::<Vec<_>>(),
                    got_o.keys().collect::<Vec<_>>(),
                    "{got}\n{step}"
                );
                for (key, w) in want {
                    self.held(w, &got_o[key], step);
                }
            }
            _ => assert_eq!(want, got, "{step}"),
        }
    }

    fn failure(&mut self, failure: &CallFailure, step: &Value) {
        let want = &step["error"];
        assert_eq!(want["code"], failure.code.as_str(), "{failure:?}\n{step}");
        assert_eq!(
            want["retryable"].as_bool().unwrap_or(false),
            failure.retryable,
            "{failure:?}\n{step}"
        );
    }

    fn serve(
        &mut self,
        surface: &str,
        asked: ServeRequest,
        at: Millis,
        held: u64,
        step: &Value,
    ) -> Result<Vec<Delivery>, ServeError> {
        let opened = self.hub.open(
            &mut self.store,
            surface,
            &asked.registration,
            asked.wait_ms,
            asked.answers,
            at,
        )?;
        if held > 0 {
            assert_eq!(opened.serve.until(), at.after(held), "{step}");
            for at in [at, at.after(held - 1)] {
                let ready = self.hub.ready(&mut self.store, &opened.serve, at)?;
                assert_eq!(ready, Ready::Wait, "{step}");
            }
        }
        self.hub
            .close(&mut self.store, opened.serve, at.after(held))
    }

    fn step(&mut self, step: &Value) {
        let at = Millis(step["at_ms"].as_u64().expect("a step has its instant"));
        let surface = step["surface"].as_str().unwrap_or("");
        match (step["host"].as_str(), step["run"].as_str()) {
            (Some(host), _) => {
                let asked = self.filled(&step["request"]);
                let answered = match host {
                    "register" => {
                        let asked: RegisterRequest = serde_json::from_value(asked).unwrap();
                        self.registered += 1;
                        let fresh = rid(self.registered);
                        self.hub
                            .register(&mut self.store, surface, &asked, &fresh, at)
                            .map(|r| serde_json::to_value(r).unwrap())
                            .map_err(|e| e.body())
                    }
                    "serve" => {
                        let asked: ServeRequest = serde_json::from_value(asked).unwrap();
                        let held = step["held_ms"].as_u64().unwrap_or(0);
                        self.serve(surface, asked, at, held, step)
                            .map(|calls| json!({ "calls": calls }))
                            .map_err(|e| e.body())
                    }
                    other => panic!("the script has a request the test does not know: {other}"),
                };
                let (status, body) = match answered {
                    Ok(body) => (200, body),
                    Err(body) => (
                        wire::status(&body.code),
                        serde_json::to_value(wire::ErrorResponse { error: body }).unwrap(),
                    ),
                };
                assert_eq!(step["status"], status, "{body}\n{step}");
                self.held(&step["response"], &body, step);
            }
            (_, Some("call")) => {
                let registration = self.filled(&step["registration"]);
                let asked = &step["call"];
                let made = self.hub.enqueue(
                    &mut self.store,
                    registration.as_str().unwrap(),
                    Enqueue {
                        run: asked["run"].as_str().unwrap().to_string(),
                        op: asked["op"].as_str().unwrap().to_string(),
                        args: asked["args"].as_array().unwrap().clone(),
                        key: asked["key"].as_str().unwrap().to_string(),
                        deadline: asked["deadline_ms"].as_u64().map(Millis),
                    },
                    at,
                );
                match made {
                    Ok(id) => {
                        assert!(step["error"].is_null(), "{id}\n{step}");
                        self.held(&step["id"], &json!(id), step);
                    }
                    Err(e) => self.failure(&e.failure(), step),
                }
            }
            (_, Some("outcome")) => {
                let id = self.filled(&step["id"]);
                self.hub.expire(&mut self.store, at).unwrap();
                let outcome = self
                    .store
                    .take_answer(id.as_str().unwrap())
                    .unwrap_or_else(|| panic!("the call is not settled\n{step}"));
                match outcome {
                    Ok(value) => {
                        assert!(step["error"].is_null(), "{value}\n{step}");
                        assert_eq!(step["result"], value, "{step}");
                    }
                    Err(failure) => self.failure(&failure, step),
                }
            }
            (_, Some("waiting")) => {
                let id = self.filled(&step["id"]);
                self.hub.expire(&mut self.store, at).unwrap();
                assert!(self.store.is_open(id.as_str().unwrap()), "{step}");
            }
            (_, Some("ended")) => {
                let registration = self.filled(&step["registration"]);
                let ended = &step["ended"];
                let id = self
                    .hub
                    .ended(
                        &mut self.store,
                        registration.as_str().unwrap(),
                        ended["run"].as_str().unwrap(),
                        serde_json::from_value(ended["end"].clone()).unwrap(),
                        ended.get("result").cloned(),
                        at,
                    )
                    .unwrap();
                self.held(&step["id"], &json!(id), step);
            }
            _ => panic!("the script has a step the test does not know: {step}"),
        }
    }
}

#[test]
fn the_fixtures_script_is_what_the_hub_does() {
    let fixture = fixture();
    let (hub, woken) = hub(HubLimits::default());
    let mut replay = Replay {
        hub,
        store: MemoryPending::new(),
        files: fixture["files"].clone(),
        names: BTreeMap::new(),
        registered: 0,
    };
    let mut last = 0;
    let script = fixture["script"]
        .as_array()
        .expect("the fixture has a script");
    for step in script {
        let at = step["at_ms"].as_u64().unwrap();
        assert!(
            at >= last,
            "the script is in the order of its clock\n{step}"
        );
        last = at;
        replay.step(step);
    }
    // Everything the script opened was settled and read, and every end
    // acknowledged.
    assert_eq!(replay.store.held(), (0, 0));
    // The hook was called for each new call and for the end, and not
    // for the call made a second time.
    let calls = script
        .iter()
        .filter(|s| s["run"] == "call" && s["error"].is_null())
        .count();
    assert_eq!(woken.lock().unwrap().len(), calls - 1 + 1);
    // The fixture's hash is the definition's, and an ID is 32
    // hexadecimal digits.
    let a = replay.names["$a"].as_str().unwrap().to_string();
    assert!(is_registration_id(&a), "{a}");
    let registration = replay.hub.registration(&a).expect("$a is registered");
    assert_eq!(script[0]["response"]["hash"], registration.hash);
    assert_eq!(registration.hash.len(), 64);
}

// ---------------------------------------------------------------------------
// One rule at a time
// ---------------------------------------------------------------------------

/// A hub with one registration of `desk`, `rid(1)`, made at 0.
fn desk(limits: HubLimits) -> (Hub, MemoryPending, Arc<Mutex<Vec<String>>>) {
    let (mut hub, woken) = hub(limits);
    let mut store = MemoryPending::new();
    let registered = hub
        .register(
            &mut store,
            "desk",
            &request(&desk_dts()),
            &rid(1),
            Millis(0),
        )
        .unwrap();
    assert_eq!(registered.registration, rid(1));
    (hub, store, woken)
}

#[test]
fn a_registration_is_connected_for_fifteen_seconds_after_a_serve_and_kept_five_minutes_more() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    // Registered at 0: connected as if a serve had just closed.
    assert!(hub.connected(&r1, Millis(15_000)));
    assert!(!hub.connected(&r1, Millis(15_001)));
    let refused = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k1", None),
            Millis(15_001),
        )
        .unwrap_err();
    assert_eq!(refused, EnqueueError::NotConnected);
    assert_eq!(refused.failure().code, CODE_UNAVAILABLE);
    assert!(refused.failure().retryable);
    // A serve that is held is connected for as long as it is held.
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(20_000))
        .unwrap();
    assert_eq!(opened.serve.until(), Millis(32_000));
    assert_eq!(opened.serve.registration(), r1);
    assert!(hub.connected(&r1, Millis(32_000)));
    assert_eq!(
        hub.close(&mut store, opened.serve, Millis(32_000)).unwrap(),
        []
    );
    assert!(hub.connected(&r1, Millis(47_000)));
    assert!(!hub.connected(&r1, Millis(47_001)));
    // One that is not closed keeps it connected for its wait and no
    // longer, and a wait is 12 s at the most.
    let opened = hub
        .open(&mut store, "desk", &r1, 600_000, Vec::new(), Millis(50_000))
        .unwrap();
    assert_eq!(opened.serve.until(), Millis(62_000));
    assert_eq!(
        hub.next_wake(&mut store, Millis(50_000)),
        Ok(Some(Millis(62_000 + 15_001)))
    );
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(61_999)),
        Ok(Ready::Wait)
    );
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(62_000)),
        Ok(Ready::Deliver)
    );
    assert!(hub.connected(&r1, Millis(62_000 + 15_000)));
    assert!(!hub.connected(&r1, Millis(62_000 + 15_001)));
    assert_eq!(hub.held().serves, 1, "it is open until it is closed");
    // Kept for the lease, then dropped, and its serve with it.
    assert_eq!(
        hub.next_wake(&mut store, Millis(62_000 + 15_001)),
        Ok(Some(Millis(62_000 + 315_001)))
    );
    assert!(!hub.connected(&r1, Millis(62_000 + 315_000)));
    assert!(hub.registration(&r1).is_some());
    assert!(!hub.connected(&r1, Millis(62_000 + 315_001)));
    assert!(hub.registration(&r1).is_none());
    assert_eq!(hub.next_wake(&mut store, Millis(400_000)), Ok(None));
    // The serve that was open is of no registration now.
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(400_000)),
        Ok(Ready::Ended)
    );
    assert_eq!(
        hub.close(&mut store, opened.serve, Millis(400_000))
            .unwrap(),
        []
    );
    assert_eq!(
        hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(400_000)),
        Err(ServeError::UnknownRegistration)
    );
}

#[test]
fn a_serve_is_answered_with_what_is_due_whether_or_not_its_wait_is_over() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let lamps = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            Millis(1),
        )
        .unwrap();
    // A serve that waits for nothing, closed an instant after it was
    // opened: a clock does not stop between the two.
    let opened = hub
        .open(&mut store, "desk", &r1, 0, Vec::new(), Millis(10))
        .unwrap();
    assert_eq!(opened.serve.until(), Millis(10));
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(11)),
        Ok(Ready::Deliver)
    );
    let sent = hub.close(&mut store, opened.serve, Millis(11)).unwrap();
    assert_eq!(ids(&sent), [lamps.as_str()]);
    // A serve held for its wait, with nothing new, asked and closed
    // after the wait was over.
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(20))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(12_019)),
        Ok(Ready::Wait)
    );
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(12_700)),
        Ok(Ready::Deliver)
    );
    let sent = hub.close(&mut store, opened.serve, Millis(12_700)).unwrap();
    assert_eq!(ids(&sent), [lamps.as_str()]);
    // Its registration was served when it was closed.
    assert!(hub.connected(&r1, Millis(12_700 + 15_000)));
    assert!(!hub.connected(&r1, Millis(12_700 + 15_001)));
    // Closed long after, while its registration is still held: the
    // same, though the registration was not connected in between.
    let opened = hub
        .open(&mut store, "desk", &r1, 0, Vec::new(), Millis(40_000))
        .unwrap();
    assert!(!hub.connected(&r1, Millis(60_000)));
    let sent = hub.close(&mut store, opened.serve, Millis(60_000)).unwrap();
    assert_eq!(ids(&sent), [lamps.as_str()]);
    assert!(hub.connected(&r1, Millis(60_001)));
    assert_eq!(hub.held().serves, 0);
}

#[test]
fn a_serve_whose_request_went_away_marks_nothing_as_sent() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let note = |hub: &mut Hub, store: &mut MemoryPending, key: &str| {
        hub.enqueue(
            store,
            &rid(1),
            call("desk.notify", json!(["hi"]), key, Some(1_000)),
            Millis(1),
        )
        .unwrap()
    };
    // Closed and answered: the effect was sent, and at its deadline may
    // have taken effect.
    let sent = note(&mut hub, &mut store, "k1");
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(2))
        .unwrap();
    assert_eq!(
        ids(&hub.close(&mut store, opened.serve, Millis(2)).unwrap()),
        [sent.as_str()]
    );
    hub.serve(
        &mut store,
        "desk",
        &r1,
        vec![result(&sent, json!(null))],
        Millis(3),
    )
    .unwrap();
    // Abandoned: it was sent to no one.
    let unsent = note(&mut hub, &mut store, "k2");
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(4))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(4)),
        Ok(Ready::Deliver)
    );
    hub.abandon(opened.serve, Millis(5));
    assert_eq!(hub.held().serves, 0);
    // The call is still new to its host.
    let opened = hub
        .open(&mut store, "desk", &r1, 0, Vec::new(), Millis(6))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(6)),
        Ok(Ready::Deliver)
    );
    hub.abandon(opened.serve, Millis(6));
    assert_eq!(
        hub.expire(&mut store, Millis(1_000)).unwrap(),
        std::slice::from_ref(&unsent)
    );
    let failure = store.take_answer(&unsent).unwrap().unwrap_err();
    assert_eq!(
        (failure.code.as_str(), failure.retryable),
        (CODE_UNAVAILABLE, true)
    );
    // The same call, closed and answered and then not answered by its
    // host, is the other error.
    let third = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.notify", json!(["hi"]), "k3", Some(2_000)),
            Millis(1_001),
        )
        .unwrap();
    hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(1_002))
        .unwrap();
    hub.expire(&mut store, Millis(2_000)).unwrap();
    assert_eq!(
        store.take_answer(&third).unwrap().unwrap_err().code,
        CODE_UNKNOWN_OUTCOME
    );
    // Abandoning a serve that has ended, or twice over, changes nothing.
    let opened = hub
        .open(&mut store, "desk", &r1, 0, Vec::new(), Millis(2_001))
        .unwrap();
    let again = Serve {
        registration: r1.clone(),
        hold: opened.serve.hold,
        until: opened.serve.until,
    };
    hub.abandon(opened.serve, Millis(2_002));
    hub.abandon(again, Millis(400_000));
    assert!(hub.registration(&r1).is_none());
}

#[test]
fn the_next_wake_is_never_a_past_instant_of_what_only_a_serve_can_take() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let id = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", Some(500_000)),
            Millis(1),
        )
        .unwrap();
    hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(2))
        .unwrap();
    let later = Answer {
        id: id.clone(),
        outcome: Outcome::RetryAfter(1_000),
    };
    hub.serve(&mut store, "desk", &r1, vec![later], Millis(3))
        .unwrap();
    // Until the call is due again, that instant is the next.
    assert_eq!(
        hub.next_wake(&mut store, Millis(3)),
        Ok(Some(Millis(1_003)))
    );
    assert_eq!(
        hub.next_wake(&mut store, Millis(1_002)),
        Ok(Some(Millis(1_003)))
    );
    // Due and not served: it waits for a serve, and the clock's next
    // instants are the registration's.
    assert_eq!(hub.held().deferred, 1);
    assert_eq!(
        hub.next_wake(&mut store, Millis(1_003)),
        Ok(Some(Millis(15_004)))
    );
    assert_eq!(
        hub.next_wake(&mut store, Millis(15_003)),
        Ok(Some(Millis(15_004)))
    );
    assert_eq!(
        hub.next_wake(&mut store, Millis(15_004)),
        Ok(Some(Millis(315_004)))
    );
    // Every instant answered is after the one asked at, up to the
    // call's deadline; an expiry at each changes nothing before it.
    let mut at = 15_004;
    while let Some(next) = hub.next_wake(&mut store, Millis(at)).unwrap() {
        assert!(next.0 > at, "{next:?} at {at}");
        at = next.0;
        let settled = hub.expire(&mut store, Millis(at)).unwrap();
        assert_eq!(settled.is_empty(), at < 500_000, "at {at}");
    }
    assert_eq!(at, 500_000);
    // A deadline that has come and is not settled is the one instant
    // answered that is not after now.
    let (mut hub, mut store, _) = desk(HubLimits::default());
    hub.enqueue(
        &mut store,
        &r1,
        call("desk.lamps", json!([]), "k", Some(100)),
        Millis(1),
    )
    .unwrap();
    assert_eq!(
        hub.next_wake(&mut store, Millis(5_000)),
        Ok(Some(Millis(100)))
    );
    hub.expire(&mut store, Millis(5_000)).unwrap();
    assert_eq!(
        hub.next_wake(&mut store, Millis(5_000)),
        Ok(Some(Millis(15_001)))
    );
}

#[test]
fn a_clock_that_goes_back_is_read_as_where_it_was() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    assert!(!hub.connected(&r1, Millis(20_000)));
    assert!(!hub.connected(&r1, Millis(5)));
    hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(5))
        .unwrap();
    let id = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", Some(20_500)),
            Millis(6),
        )
        .unwrap();
    // Sent with what is left from the latest instant the hub was given.
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(7))
        .unwrap();
    let Delivery::Call(sent) = &sent[0] else {
        panic!()
    };
    assert_eq!(
        (sent.id.as_str(), sent.expires_in_ms),
        (id.as_str(), Some(500))
    );
    // And settled when that instant, not the one given, says so.
    assert_eq!(
        hub.expire(&mut store, Millis(20_500)).unwrap(),
        std::slice::from_ref(&id)
    );
    let late = hub.open(
        &mut store,
        "desk",
        &r1,
        0,
        vec![result(&id, json!([]))],
        Millis(8),
    );
    assert_eq!(late.unwrap().ignored, 1);
}

#[test]
fn a_held_serve_is_ready_when_something_is_new_to_its_host() {
    let (mut hub, mut store, woken) = desk(HubLimits::default());
    let r1 = rid(1);
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(10))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(11)),
        Ok(Ready::Wait)
    );
    let id = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            Millis(12),
        )
        .unwrap();
    assert_eq!(*woken.lock().unwrap(), std::slice::from_ref(&r1));
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(12)),
        Ok(Ready::Deliver)
    );
    let sent = hub.close(&mut store, opened.serve, Millis(12)).unwrap();
    assert_eq!(ids(&sent), [id.as_str()]);
    // A call given no deadline has a day.
    let Delivery::Call(sent) = &sent[0] else {
        panic!()
    };
    assert_eq!(sent.expires_in_ms, Some(86_400_000));
    assert_eq!(
        hub.next_wake(&mut store, Millis(12)),
        Ok(Some(Millis(12 + 15_000 + 1)))
    );
    // Sent before and not answered: it goes out with the next serve,
    // and that serve does not return early for it.
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(13))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(14)),
        Ok(Ready::Wait)
    );
    let again = hub.close(&mut store, opened.serve, Millis(12_013)).unwrap();
    assert_eq!(ids(&again), [id.as_str()]);
    // An ended run is new once.
    let stopped = End::Stopped {
        reason: "cancelled".into(),
    };
    let end = hub
        .ended(&mut store, &r1, "run-1", stopped, None, Millis(12_014))
        .unwrap();
    assert_eq!(woken.lock().unwrap().len(), 2);
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(12_015))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(12_015)),
        Ok(Ready::Deliver)
    );
    // What is new goes first.
    let sent = hub.close(&mut store, opened.serve, Millis(12_015)).unwrap();
    assert_eq!(ids(&sent), [end.as_str(), id.as_str()]);
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(12_016))
        .unwrap();
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(12_016)),
        Ok(Ready::Wait)
    );
    // A second end of the run is the first, and wakes no one.
    let second = hub
        .ended(
            &mut store,
            &r1,
            "run-1",
            End::Finished {},
            None,
            Millis(12_017),
        )
        .unwrap();
    assert_eq!(second, end);
    assert_eq!(woken.lock().unwrap().len(), 2);
    // A call its host asks for later is new to it again when that
    // comes, and not before.
    hub.close(&mut store, opened.serve, Millis(12_018)).unwrap();
    let later = Answer {
        id: id.clone(),
        outcome: Outcome::RetryAfter(1_000),
    };
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, vec![later], Millis(12_019))
        .unwrap();
    assert_eq!(hub.held().deferred, 1);
    assert_eq!(
        hub.next_wake(&mut store, Millis(12_019)),
        Ok(Some(Millis(13_019)))
    );
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(13_018)),
        Ok(Ready::Wait)
    );
    assert_eq!(
        hub.ready(&mut store, &opened.serve, Millis(13_019)),
        Ok(Ready::Deliver)
    );
    let sent = hub.close(&mut store, opened.serve, Millis(13_019)).unwrap();
    assert_eq!(ids(&sent), [id.as_str(), end.as_str()]);
    assert_eq!(hub.held().deferred, 0);
}

#[test]
fn one_serve_more_than_a_registration_may_have_open_ends_the_oldest() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let mut serves: Vec<Serve> = (0..5)
        .map(|n| {
            hub.open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(n))
                .unwrap()
                .serve
        })
        .collect();
    assert_eq!(hub.held().serves, 4);
    // The first is told so, and the others wait.
    assert_eq!(
        hub.ready(&mut store, &serves[0], Millis(5)),
        Ok(Ready::Ended)
    );
    for serve in &serves[1..] {
        assert_eq!(hub.ready(&mut store, serve, Millis(5)), Ok(Ready::Wait));
    }
    let id = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            Millis(6),
        )
        .unwrap();
    // The one that ended is answered with nothing; each that is open,
    // with everything due: a host tells a call it was sent twice by
    // its ID.
    let first = serves.remove(0);
    assert_eq!(hub.close(&mut store, first, Millis(7)).unwrap(), []);
    for serve in serves {
        assert_eq!(
            ids(&hub.close(&mut store, serve, Millis(7)).unwrap()),
            [id.as_str()]
        );
    }
    assert_eq!(hub.held().serves, 0);
}

#[test]
fn every_limit_is_an_error_with_its_name() {
    let limits = HubLimits {
        prompt_bytes: 8,
        calls_per_registration: 2,
        ended_per_registration: 1,
        args_bytes: 32,
        result_bytes: 32,
        message_bytes: 5,
        id_bytes: 8,
        answers_per_serve: 2,
        deliveries_per_serve: 1,
        serves_per_registration: 2,
        ..HubLimits::default()
    };
    let (mut hub, mut store, _) = desk(limits);
    let now = Millis(1);
    let (r1, r2) = (rid(1), rid(2));
    let dts = desk_dts();
    hub.register(&mut store, "desk", &request(&dts), &r2, now)
        .unwrap();

    // Registering.
    let wordy = RegisterRequest {
        prompt: Some("123456789".into()),
        ..request(&dts)
    };
    let refused = hub
        .register(&mut store, "desk", &wordy, &rid(3), now)
        .unwrap_err();
    assert_eq!(
        refused,
        RegisterError::PromptTooLarge { bytes: 9, limit: 8 }
    );
    assert_eq!(refused.body().code, wire::CODE_PROMPT_TOO_LARGE);
    let other = RegisterRequest {
        clax_surface: 2,
        ..request(&dts)
    };
    let refused = hub
        .register(&mut store, "desk", &other, &rid(3), now)
        .unwrap_err();
    assert_eq!(refused.body().code, wire::CODE_UNSUPPORTED_VERSION);
    assert_eq!(refused.body().supported, [1]);
    let patterns = Settings {
        allow: Some(vec!["desk.*".into(), "not a pattern".into()]),
        ..Settings::default()
    };
    let unruly = RegisterRequest {
        settings: patterns,
        ..request(&dts)
    };
    assert_eq!(
        hub.register(&mut store, "desk", &unruly, &rid(3), now)
            .unwrap_err()
            .body()
            .code,
        wire::CODE_INVALID_SETTINGS
    );
    assert_eq!(
        hub.register(&mut store, "shelf", &request(&dts), &rid(3), now)
            .unwrap_err()
            .body()
            .code,
        wire::CODE_NAME_MISMATCH
    );
    // An ID that is not 128 bits as hexadecimal digits, or that was
    // given before, was not drawn as one must be.
    for fresh in [
        "",
        "r3",
        &rid(3)[1..],
        &rid(3).to_uppercase().replace('3', "A"),
        &r1,
    ] {
        let refused = hub
            .register(&mut store, "desk", &request(&dts), fresh, now)
            .unwrap_err();
        assert_eq!(refused, RegisterError::FreshInvalid, "{fresh}");
        assert_eq!(refused.body().code, wire::CODE_INTERNAL_ERROR);
    }
    assert_eq!(
        hub.held().registrations,
        2,
        "nothing refused was registered"
    );

    // Calls.
    let enqueue =
        |hub: &mut Hub, store: &mut MemoryPending, c: Enqueue| hub.enqueue(store, &rid(1), c, now);
    let long = "x".repeat(40);
    assert_eq!(
        enqueue(
            &mut hub,
            &mut store,
            call("desk.notify", json!([long]), "k", None)
        ),
        Err(EnqueueError::ArgsTooLarge { limit: 32 })
    );
    for (run, key) in [
        ("", "k"),
        ("run-1", ""),
        ("run-1", "123456789"),
        ("123456789", "k"),
    ] {
        let bad = Enqueue {
            run: run.into(),
            ..call("desk.lamps", json!([]), key, None)
        };
        let refused = enqueue(&mut hub, &mut store, bad).unwrap_err();
        assert_eq!(refused, EnqueueError::IdTooLong { limit: 8 });
        assert_eq!(refused.failure().code, wire::CODE_INTERNAL_ERROR);
    }
    assert_eq!(
        enqueue(
            &mut hub,
            &mut store,
            call("desk.lamps", json!([]), "k", Some(1))
        ),
        Err(EnqueueError::PastDeadline)
    );
    assert_eq!(
        enqueue(
            &mut hub,
            &mut store,
            call("desk.dust", json!([]), "k", None)
        ),
        Err(EnqueueError::UnknownOp("desk.dust".into()))
    );
    assert_eq!(
        enqueue(&mut hub, &mut store, call("lamps", json!([]), "k", None)),
        Err(EnqueueError::UnknownOp("lamps".into()))
    );
    assert_eq!(
        hub.enqueue(
            &mut store,
            &rid(9),
            call("desk.lamps", json!([]), "k", None),
            now
        ),
        Err(EnqueueError::NoRegistration)
    );
    assert_eq!(store.held(), (0, 0), "a refused call leaves nothing");
    let first = enqueue(
        &mut hub,
        &mut store,
        call("desk.lamps", json!([]), "k1", None),
    )
    .unwrap();
    let second = enqueue(
        &mut hub,
        &mut store,
        call("desk.notify", json!(["hi"]), "k2", None),
    )
    .unwrap();
    let full = enqueue(
        &mut hub,
        &mut store,
        call("desk.lamps", json!([]), "k3", None),
    )
    .unwrap_err();
    assert_eq!(full, EnqueueError::QueueFull { limit: 2 });
    assert_eq!(full.failure().code, CODE_UNAVAILABLE);
    // A full queue still answers for a call it holds, and refuses
    // another under its key as that.
    assert_eq!(
        enqueue(
            &mut hub,
            &mut store,
            call("desk.notify", json!(["hi"]), "k2", None)
        ),
        Ok(second.clone())
    );
    assert_eq!(
        enqueue(
            &mut hub,
            &mut store,
            call("desk.notify", json!(["ho"]), "k2", None)
        ),
        Err(EnqueueError::KeyReused)
    );
    // Another registration's queue is its own.
    hub.enqueue(
        &mut store,
        &r2,
        call("desk.lamps", json!([]), "k3", None),
        now,
    )
    .unwrap();

    // One serve is answered with one delivery: the oldest that is new,
    // and when none is new, the oldest.
    for want in [&first, &second, &first] {
        let sent = hub.serve(&mut store, "desk", &r1, Vec::new(), now).unwrap();
        assert_eq!(ids(&sent), [want.as_str()]);
    }

    // Answers.
    let three = vec![
        result(&first, json!([])),
        result(&second, json!(null)),
        result("x", json!(1)),
    ];
    let refused = hub.serve(&mut store, "desk", &r1, three, now).unwrap_err();
    assert_eq!(refused, ServeError::TooManyAnswers { limit: 2 });
    assert_eq!(refused.body().code, wire::CODE_TOO_MANY_ANSWERS);
    assert!(
        store.is_open(&first) && store.is_open(&second),
        "none of them was taken"
    );
    let lamps: Vec<Value> = (0..4)
        .map(|i| json!({"id": format!("lamp-{i}"), "on": true}))
        .collect();
    let answers = vec![
        result(&first, json!(lamps)),
        error(&second, "unavailable", "aaaaé"),
    ];
    let opened = hub.open(&mut store, "desk", &r1, 0, answers, now).unwrap();
    assert_eq!(
        (opened.ignored, &opened.settled),
        (0, &vec![first.clone(), second.clone()])
    );
    let too_large = store.take_answer(&first).unwrap().unwrap_err();
    assert_eq!(too_large.code, CODE_INVALID_RESULT);
    assert!(too_large.message.contains("32 bytes"), "{too_large:?}");
    // The message is cut at a character's end.
    let gone = store.take_answer(&second).unwrap().unwrap_err();
    assert_eq!(
        (gone.code.as_str(), gone.message.as_str()),
        ("unavailable", "aaaa")
    );
    hub.close(&mut store, opened.serve, now).unwrap();

    // Ended runs.
    let summary = Some(json!({"summary": "ok"}));
    let end = hub
        .ended(&mut store, &r1, "run-1", End::Finished {}, summary, now)
        .unwrap();
    assert_eq!(
        hub.ended(&mut store, &r1, "run-2", End::Finished {}, None, now),
        Err(EndedError::QueueFull { limit: 1 })
    );
    // A full queue still answers for the end it holds.
    assert_eq!(
        hub.ended(&mut store, &r1, "run-1", End::Finished {}, None, now),
        Ok(end)
    );
    assert_eq!(
        hub.ended(
            &mut store,
            &r2,
            "run-2",
            End::Finished {},
            Some(json!({"summary": long})),
            now
        ),
        Err(EndedError::ResultTooLarge { limit: 32 })
    );
    for run in ["", "123456789"] {
        assert_eq!(
            hub.ended(&mut store, &r2, run, End::Finished {}, None, now),
            Err(EndedError::IdTooLong { limit: 8 })
        );
    }
    for never in [rid(9), "not an id".to_string()] {
        assert_eq!(
            hub.ended(&mut store, &never, "run-2", End::Finished {}, None, now),
            Err(EndedError::NoRegistration)
        );
    }
    assert_eq!(store.held().1, 1);
}

#[test]
fn at_a_limit_the_oldest_registration_that_is_not_connected_makes_room() {
    let limits = HubLimits {
        registrations: 4,
        registrations_per_name: 3,
        ..HubLimits::default()
    };
    let (mut hub, _) = hub(limits);
    let mut store = MemoryPending::new();
    let (desk, shelf) = (request(&desk_dts()), request(&shelf_dts()));
    // Three of `desk`, at 0, 1 and 2, the second of them served since.
    for n in 1..=3 {
        hub.register(&mut store, "desk", &desk, &rid(n), Millis(n - 1))
            .unwrap();
    }
    let call = hub
        .enqueue(
            &mut store,
            &rid(1),
            call("desk.lamps", json!([]), "k", Some(900_000)),
            Millis(3),
        )
        .unwrap();
    hub.serve(&mut store, "desk", &rid(2), Vec::new(), Millis(10_000))
        .unwrap();
    // While every one is connected, a fourth is refused: at 15,000 the
    // first still is.
    let full = hub
        .register(&mut store, "desk", &desk, &rid(4), Millis(15_000))
        .unwrap_err();
    assert_eq!(
        full,
        RegisterError::TooManyRegistrations {
            limit: 3,
            of_name: true
        }
    );
    assert_eq!(full.body().code, wire::CODE_TOO_MANY_REGISTRATIONS);
    assert_eq!(wire::status(&full.body().code), 429);
    assert_eq!(hub.held().registrations, 3);
    // A millisecond later it is not, and goes.
    hub.register(&mut store, "desk", &desk, &rid(4), Millis(15_001))
        .unwrap();
    assert!(hub.registration(&rid(1)).is_none());
    assert!(hub.registration(&rid(3)).is_some());
    // What it had waits in the store, and it is gone on with by its
    // ID, which makes room in its turn.
    assert!(store.is_open(&call));
    hub.register(
        &mut store,
        "desk",
        &again(1, &desk_dts()),
        &rid(5),
        Millis(15_003),
    )
    .unwrap();
    assert!(hub.registration(&rid(3)).is_none());
    let sent = hub
        .serve(&mut store, "desk", &rid(1), Vec::new(), Millis(15_004))
        .unwrap();
    assert_eq!(ids(&sent), [call.as_str()]);
    // The limit on all of them: `shelf` has room of its own, and the
    // fourth and fifth registrations of any name each take the oldest
    // that is not connected, of whatever name.
    hub.register(&mut store, "shelf", &shelf, &rid(6), Millis(15_005))
        .unwrap();
    assert_eq!(hub.held().registrations, 4);
    hub.register(&mut store, "shelf", &shelf, &rid(7), Millis(40_000))
        .unwrap();
    assert_eq!(hub.held().registrations, 4);
    assert!(hub.registration(&rid(2)).is_none(), "served last at 10,000");
    // With all four connected, none goes.
    for (name, n) in [("desk", 4), ("desk", 1), ("shelf", 6), ("shelf", 7)] {
        hub.serve(&mut store, name, &rid(n), Vec::new(), Millis(40_001))
            .unwrap();
    }
    assert_eq!(
        hub.register(&mut store, "shelf", &shelf, &rid(8), Millis(40_002)),
        Err(RegisterError::TooManyRegistrations {
            limit: 4,
            of_name: false
        })
    );
    // Going on as one that is held takes no room.
    hub.register(
        &mut store,
        "shelf",
        &again(6, &shelf_dts()),
        &rid(8),
        Millis(40_003),
    )
    .unwrap();
    assert_eq!(hub.held().registrations, 4);
}

#[test]
fn a_serve_is_answered_with_no_more_bytes_than_the_limit_and_one_delivery_at_the_least() {
    let limits = HubLimits {
        deliveries_bytes: 100,
        ..HubLimits::default()
    };
    let (mut hub, mut store, _) = desk(limits);
    let r1 = rid(1);
    let text = |n: usize| json!(["n".repeat(n)]);
    let mut made = Vec::new();
    for (n, size) in [(0, 150), (1, 40), (2, 40), (3, 40)] {
        let key = format!("k{n}");
        made.push(
            hub.enqueue(
                &mut store,
                &r1,
                call("desk.notify", text(size), &key, None),
                Millis(1),
            )
            .unwrap(),
        );
    }
    hub.ended(
        &mut store,
        &r1,
        "run-1",
        End::Finished {},
        Some(json!({"summary": "s".repeat(80)})),
        Millis(1),
    )
    .unwrap();
    // One that is larger than the limit goes alone.
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(2))
        .unwrap();
    assert_eq!(ids(&sent), [made[0].as_str()]);
    // Then as many as fit: two of 44 bytes, and not a third.
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(3))
        .unwrap();
    assert_eq!(ids(&sent), [made[1].as_str(), made[2].as_str()]);
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(4))
        .unwrap();
    assert_eq!(
        sent.len(),
        1,
        "the last call, and not the end with it: {sent:?}"
    );
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(5))
        .unwrap();
    assert!(matches!(&sent[..], [Delivery::Ended(_)]), "{sent:?}");
}

#[test]
fn calls_and_ends_do_not_wait_for_ever() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    const DAY: u64 = 86_400_000;
    // A call given no deadline has a day from when it is made.
    let id = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            Millis(1_000),
        )
        .unwrap();
    assert_eq!(store.next_deadline(), Ok(Some(Millis(1_000 + DAY))));
    assert_eq!(
        hub.next_wake(&mut store, Millis(1_000)),
        Ok(Some(Millis(15_001)))
    );
    let end = hub
        .ended(
            &mut store,
            &r1,
            "run-1",
            End::Finished {},
            None,
            Millis(2_000),
        )
        .unwrap();
    assert_eq!(
        hub.expire(&mut store, Millis(1_000 + DAY - 1)).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        hub.next_wake(&mut store, Millis(1_000 + DAY - 1)),
        Ok(Some(Millis(1_000 + DAY)))
    );
    assert_eq!(
        hub.expire(&mut store, Millis(1_000 + DAY)).unwrap(),
        std::slice::from_ref(&id)
    );
    assert_eq!(
        store.take_answer(&id).unwrap().unwrap_err().code,
        CODE_UNAVAILABLE
    );
    assert_eq!(hub.next_wake(&mut store, Millis(1_000 + DAY)), Ok(None));
    // An end is kept seven days for its host, and then forgotten.
    hub.expire(&mut store, Millis(2_000 + 7 * DAY)).unwrap();
    assert_eq!(store.held().1, 1);
    hub.expire(&mut store, Millis(2_001 + 7 * DAY)).unwrap();
    assert_eq!(store.held().1, 0);
    // Its host, back after that, is sent nothing and its answer names
    // nothing.
    hub.register(
        &mut store,
        "desk",
        &again(1, &desk_dts()),
        &rid(2),
        Millis(8 * DAY),
    )
    .unwrap();
    let opened = hub
        .open(
            &mut store,
            "desk",
            &r1,
            0,
            vec![result(&end, json!(null))],
            Millis(8 * DAY),
        )
        .unwrap();
    assert_eq!(opened.ignored, 1);
    assert_eq!(
        hub.close(&mut store, opened.serve, Millis(8 * DAY))
            .unwrap(),
        []
    );
}

/// What a serve of `r` with `answers` at `at` ignored, and the IDs it
/// was answered with.
fn served(
    hub: &mut Hub,
    store: &mut MemoryPending,
    r: &str,
    answers: Vec<Answer>,
    at: u64,
) -> (usize, String) {
    let opened = hub.open(store, "desk", r, 0, answers, Millis(at)).unwrap();
    let ignored = opened.ignored;
    let sent = hub.close(store, opened.serve, Millis(at)).unwrap();
    (ignored, ids(&sent).join(","))
}

#[test]
fn an_answer_settles_its_own_registrations_open_call_once() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let (r1, r2) = (rid(1), rid(2));
    hub.register(&mut store, "desk", &request(&desk_dts()), &r2, Millis(0))
        .unwrap();
    let ask = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.ask", json!(["Tidy?", null]), "k1", Some(100)),
            Millis(1),
        )
        .unwrap();
    // Another registration's answer, in every form, changes nothing.
    for outcome in [
        Outcome::Result(json!("yes")),
        Outcome::Accepted,
        Outcome::RetryAfter(50),
        Outcome::Error(CallError {
            code: "declined".into(),
            message: String::new(),
        }),
    ] {
        let answer = Answer {
            id: ask.clone(),
            outcome,
        };
        assert_eq!(
            served(&mut hub, &mut store, &r2, vec![answer], 2),
            (1, String::new())
        );
    }
    assert_eq!(
        served(&mut hub, &mut store, &r1, Vec::new(), 3),
        (0, ask.clone())
    );
    // An ID longer than any the store gives is not looked for.
    let long = result(&"c".repeat(300), json!(1));
    assert_eq!(served(&mut hub, &mut store, &r1, vec![long], 4).0, 1);
    // The first result or error for a call stands, and a second is
    // ignored, in that request and in a later one.
    let both = vec![error(&ask, "declined", "no"), result(&ask, json!("yes"))];
    assert_eq!(served(&mut hub, &mut store, &r1, both, 5).0, 1);
    assert_eq!(
        served(
            &mut hub,
            &mut store,
            &r1,
            vec![result(&ask, json!("yes"))],
            6
        )
        .0,
        1
    );
    let failure = store.take_answer(&ask).unwrap().unwrap_err();
    assert_eq!(
        (failure.code.as_str(), failure.message.as_str()),
        ("declined", "no")
    );
    // A call no serve was answered with, which its host says it has,
    // counts as sent when its deadline comes.
    let set = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.settings.set", json!([{}]), "k2", Some(100)),
            Millis(7),
        )
        .unwrap();
    let accepted = Answer {
        id: set.clone(),
        outcome: Outcome::Accepted,
    };
    assert_eq!(
        served(&mut hub, &mut store, &r1, vec![accepted], 8),
        (0, String::new())
    );
    assert_eq!(
        hub.expire(&mut store, Millis(99)).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        hub.expire(&mut store, Millis(100)).unwrap(),
        std::slice::from_ref(&set)
    );
    assert_eq!(
        hub.expire(&mut store, Millis(101)).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        store.take_answer(&set).unwrap().unwrap_err().code,
        CODE_UNKNOWN_OUTCOME
    );
}

#[test]
fn accepted_and_retry_after_do_not_keep_a_result_in_the_same_serve_from_settling() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let ask = |hub: &mut Hub, store: &mut MemoryPending, n: u64| {
        let key = format!("k{n}");
        hub.enqueue(
            store,
            &rid(1),
            call("desk.ask", json!(["Tidy?", null]), &key, None),
            Millis(n),
        )
        .unwrap()
    };
    let accepted = |id: &str| Answer {
        id: id.into(),
        outcome: Outcome::Accepted,
    };
    let later = |id: &str| Answer {
        id: id.into(),
        outcome: Outcome::RetryAfter(60_000),
    };
    // Accepted, then its result.
    let a = ask(&mut hub, &mut store, 1);
    let opened = hub
        .open(
            &mut store,
            "desk",
            &r1,
            0,
            vec![accepted(&a), result(&a, json!("yes"))],
            Millis(2),
        )
        .unwrap();
    assert_eq!((opened.ignored, &opened.settled), (0, &vec![a.clone()]));
    assert_eq!(store.take_answer(&a).unwrap(), Ok(json!("yes")));
    // Asked for later, accepted twice, then an error; what follows the
    // error for that call is ignored, whatever it is.
    let b = ask(&mut hub, &mut store, 3);
    let answers = vec![
        later(&b),
        accepted(&b),
        accepted(&b),
        error(&b, "declined", "no"),
        result(&b, json!("yes")),
        accepted(&b),
        later(&b),
    ];
    let opened = hub
        .open(&mut store, "desk", &r1, 0, answers, Millis(4))
        .unwrap();
    assert_eq!((opened.ignored, &opened.settled), (3, &vec![b.clone()]));
    assert_eq!(store.take_answer(&b).unwrap().unwrap_err().code, "declined");
    assert_eq!(hub.held().deferred, 0);
    // A result in a later serve settles a call that was accepted.
    let c = ask(&mut hub, &mut store, 5);
    assert_eq!(
        served(&mut hub, &mut store, &r1, vec![accepted(&c)], 6),
        (0, String::new())
    );
    assert_eq!(
        served(&mut hub, &mut store, &r1, vec![result(&c, json!("no"))], 7).0,
        0
    );
    assert_eq!(store.take_answer(&c).unwrap(), Ok(json!("no")));
}

#[test]
fn a_host_throws_what_its_operation_declares_or_unavailable_and_nothing_else() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let mut n = 0;
    let mut thrown = |op: &str, args: Value, code: &str| {
        n += 1;
        let id = hub
            .enqueue(
                &mut store,
                &r1,
                call(op, args, &format!("k{n}"), None),
                Millis(n),
            )
            .unwrap();
        let answers = vec![error(&id, code, "the host's words")];
        hub.serve(&mut store, "desk", &r1, answers, Millis(n))
            .unwrap();
        store.take_answer(&id).unwrap().unwrap_err()
    };
    // `ask` declares `declined`, `settings.set` declares `invalid`.
    let declined = thrown("desk.ask", json!(["Tidy?", null]), "declined");
    assert_eq!(
        (
            declined.code.as_str(),
            declined.message.as_str(),
            declined.retryable
        ),
        ("declined", "the host's words", false)
    );
    assert_eq!(
        thrown("desk.settings.set", json!([{}]), "invalid").code,
        "invalid"
    );
    let unavailable = thrown("desk.lamps", json!([]), "unavailable");
    assert_eq!(
        (unavailable.code.as_str(), unavailable.retryable),
        ("unavailable", true)
    );
    // Another operation's code, the core's own codes, and what is no
    // code at all: `invalid_result`, which says what the host said.
    for (op, args, code) in [
        ("desk.ask", json!(["Tidy?", null]), "invalid"),
        ("desk.lamps", json!([]), "declined"),
        ("desk.lamps", json!([]), "invalid_result"),
        ("desk.lamps", json!([]), "expired"),
        ("desk.lamps", json!([]), "unknown_outcome"),
        ("desk.lamps", json!([]), "invalid_args"),
        ("desk.lamps", json!([]), "unknown_op"),
        ("desk.lamps", json!([]), "key_reused"),
        ("desk.lamps", json!([]), "internal_error"),
        ("desk.lamps", json!([]), "Declined!"),
        ("desk.lamps", json!([]), ""),
    ] {
        let failure = thrown(op, args, code);
        assert_eq!(failure.code, CODE_INVALID_RESULT, "{op} {code}");
        assert!(!failure.retryable);
        assert!(
            failure.message.contains(&format!("`{code}`"))
                && failure.message.contains("the host's words"),
            "{failure:?}"
        );
    }
}

#[test]
fn a_key_is_one_calls_for_as_long_as_the_store_holds_the_call() {
    let (mut hub, mut store, woken) = desk(HubLimits::default());
    let r1 = rid(1);
    let note = |text: &str| call("desk.notify", json!([text]), "k", None);
    let id = hub
        .enqueue(&mut store, &r1, note("one"), Millis(1))
        .unwrap();
    hub.serve(
        &mut store,
        "desk",
        &r1,
        vec![result(&id, json!(null))],
        Millis(2),
    )
    .unwrap();
    // Settled and not yet read: the same call is that call, and
    // another under its key is refused, as while it was open.
    assert_eq!(
        hub.enqueue(&mut store, &r1, note("one"), Millis(3)),
        Ok(id.clone())
    );
    for other in [
        note("two"),
        call("desk.lamps", json!([]), "k", None),
        Enqueue {
            run: "run-2".into(),
            ..note("one")
        },
    ] {
        let refused = hub.enqueue(&mut store, &r1, other, Millis(3)).unwrap_err();
        assert_eq!(refused, EnqueueError::KeyReused);
        assert_eq!(refused.failure().code, CODE_KEY_REUSED);
    }
    hub.register(
        &mut store,
        "desk",
        &request(&desk_dts()),
        &rid(2),
        Millis(3),
    )
    .unwrap();
    assert_eq!(
        hub.enqueue(&mut store, &rid(2), note("one"), Millis(3)),
        Err(EnqueueError::KeyReused)
    );
    assert_eq!(
        woken.lock().unwrap().len(),
        1,
        "only the call that was new woke its host"
    );
    // Read, the store has forgotten it, and the key is free.
    assert_eq!(store.take_answer(&id).unwrap(), Ok(json!(null)));
    let next = hub
        .enqueue(&mut store, &r1, note("two"), Millis(4))
        .unwrap();
    assert_ne!(next, id);
}

#[test]
fn an_operation_whose_types_reach_content_is_withheld() {
    let dts = r#"declare const shelf: {
  /** The books. @read */
  books(): Promise<Book[]>;
  /** A book's cover. @read */
  cover(book: string): Promise<{ front: Image } | null>;
  scans: {
    /** Files a scan. @write */
    file(scan: Scan): Promise<null>;
  };
  /** How many books. @read */
  count(): Promise<Int>;
  /** A book's spine, which no kind allows. @read */
  spine(book: string): Promise<Image>;
};
interface Book extends Bound { title: string }
interface Bound { spine?: Record<string, Content[]> }
interface Scan { pages: Page[] }
interface Page { n: Int; next?: Page; image: Image }
interface Kinds {
  /** Sorts the shelf. @allow shelf.books shelf.count */
  sort(request: { ask: Instruction }): { summary: string };
  /** Scans a book. @allow shelf.cover shelf.scans.* shelf.count */
  scan(request: { ask: Instruction }): { summary: string };
}
"#;
    let (mut hub, _) = hub(HubLimits::default());
    let mut store = MemoryPending::new();
    let registered = hub
        .register(&mut store, "shelf", &request(dts), &rid(1), Millis(0))
        .unwrap();
    let withheld: Vec<(&str, &str)> = registered
        .withheld
        .iter()
        .map(|w| (w.op.as_str(), w.reason.as_str()))
        .collect();
    // In the definition's order; `spine`, which no run has in any
    // case, is not among them.
    assert_eq!(
        withheld,
        [
            ("books", "content"),
            ("cover", "content"),
            ("scans.file", "content")
        ]
    );
    assert_eq!(
        hub.registration(&rid(1)).unwrap().withheld,
        registered.withheld
    );
    let filed = call("shelf.scans.file", json!([{"pages": []}]), "k", None);
    let refused = hub
        .enqueue(&mut store, &rid(1), filed, Millis(1))
        .unwrap_err();
    assert_eq!(refused, EnqueueError::Withheld("shelf.scans.file".into()));
    assert_eq!(refused.failure().code, CODE_UNKNOWN_OP);
    hub.enqueue(
        &mut store,
        &rid(1),
        call("shelf.count", json!([]), "k", None),
        Millis(1),
    )
    .unwrap();
}

/// The fixture's surface with `lamps` gone and `ask` answering a
/// number.
fn changed_dts() -> String {
    let changed = desk_dts()
        .replace(
            "  /** The lamps on the desk. @read */\n  lamps(): Promise<Lamp[]>;\n",
            "",
        )
        .replace("interface Lamp { id: string; on: boolean }\n", "")
        .replace(
            "choices?: string[]): Promise<string>",
            "choices?: string[]): Promise<Int>",
        );
    assert!(
        !changed.contains("amp") && changed.contains("Promise<Int>"),
        "{changed}"
    );
    changed
}

#[test]
fn only_an_id_this_daemon_gave_a_registration_of_the_surface_is_gone_on_with() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let lamps = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k", Some(900_000)),
            Millis(1),
        )
        .unwrap();
    let (desk, shelf) = (desk_dts(), shelf_dts());
    let unknown =
        |hub: &mut Hub, store: &mut MemoryPending, name: &str, r: RegisterRequest, at: u64| {
            let refused = hub
                .register(store, name, &r, &rid(7), Millis(at))
                .unwrap_err();
            assert_eq!(
                refused,
                RegisterError::UnknownRegistration,
                "{name} at {at}"
            );
            assert_eq!(refused.body().code, wire::CODE_UNKNOWN_REGISTRATION);
            assert_eq!(wire::status(&refused.body().code), 404);
        };
    // While the hub holds it, and once it has dropped it: an ID of
    // another surface, an ID nothing issued, and what is no ID.
    for at in [2, 400_000] {
        unknown(&mut hub, &mut store, "shelf", again(1, &shelf), at);
        unknown(&mut hub, &mut store, "desk", again(5, &desk), at);
        for odd in ["my-constant-id", "", &"A".repeat(32), &rid(1)[1..]] {
            let named = RegisterRequest {
                registration: Some(odd.into()),
                ..request(&desk)
            };
            unknown(&mut hub, &mut store, "desk", named, at);
        }
    }
    assert!(hub.registration(&r1).is_none(), "its lease has ended");
    assert_eq!(hub.held().registrations, 0);
    // A registration of `shelf` is sent nothing of `desk`'s.
    hub.register(
        &mut store,
        "shelf",
        &request(&shelf),
        &rid(2),
        Millis(400_001),
    )
    .unwrap();
    assert_eq!(
        hub.serve(&mut store, "shelf", &rid(2), Vec::new(), Millis(400_002))
            .unwrap(),
        []
    );
    // The ID, for its own surface, is gone on with, and has its call.
    let registered = hub
        .register(
            &mut store,
            "desk",
            &again(1, &desk),
            &rid(3),
            Millis(400_003),
        )
        .unwrap();
    assert_eq!(registered.registration, r1);
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(400_004))
        .unwrap();
    assert_eq!(ids(&sent), [lamps.as_str()]);
    // The ID that was drawn for it and not used is no registration.
    unknown(&mut hub, &mut store, "desk", again(3, &desk), 400_005);
}

#[test]
fn an_answer_is_checked_by_the_definition_its_call_was_made_under() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let lamps = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k1", None),
            Millis(1),
        )
        .unwrap();
    let ask = |hub: &mut Hub, store: &mut MemoryPending, key: &str, at: u64| {
        hub.enqueue(
            store,
            &rid(1),
            call("desk.ask", json!(["Tidy?", null]), key, None),
            Millis(at),
        )
        .unwrap()
    };
    let old_ask = ask(&mut hub, &mut store, "k2", 1);
    let old_ask2 = ask(&mut hub, &mut store, "k3", 1);
    let opened = hub
        .open(&mut store, "desk", &r1, 12_000, Vec::new(), Millis(2))
        .unwrap();
    // The host registers again as itself, with a definition in which
    // `ask` answers a number and `lamps` is gone.
    let before = hub.registration(&r1).unwrap().hash.to_string();
    let registered = hub
        .register(
            &mut store,
            "desk",
            &again(1, &changed_dts()),
            &rid(2),
            Millis(3),
        )
        .unwrap();
    assert_eq!(registered.registration, r1);
    assert_ne!(registered.hash, before);
    assert_eq!(
        hub.held(),
        Held {
            registrations: 1,
            serves: 1,
            deferred: 0,
            definitions: 2
        }
    );
    // The open serve is still its serve, and the calls made before are
    // sent as they were made.
    let sent = hub.close(&mut store, opened.serve, Millis(4)).unwrap();
    assert_eq!(
        ids(&sent),
        [lamps.as_str(), old_ask.as_str(), old_ask2.as_str()]
    );
    assert!(matches!(&sent[0], Delivery::Call(c) if c.op == "lamps"));
    // New calls are made under the new definition.
    assert_eq!(
        hub.enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "k4", None),
            Millis(5)
        ),
        Err(EnqueueError::UnknownOp("desk.lamps".into()))
    );
    let new_ask = ask(&mut hub, &mut store, "k5", 5);
    // Each answer is held to its own call's definition: the old ones
    // take lamps and a string, the new one a number.
    let answers = vec![
        result(&lamps, json!([{"id": "l1", "on": false}])),
        result(&old_ask, json!("yes")),
        result(&old_ask2, json!(7)),
        result(&new_ask, json!(7)),
    ];
    hub.serve(&mut store, "desk", &r1, answers, Millis(6))
        .unwrap();
    assert_eq!(
        store.take_answer(&lamps).unwrap(),
        Ok(json!([{"id": "l1", "on": false}]))
    );
    assert_eq!(store.take_answer(&old_ask).unwrap(), Ok(json!("yes")));
    assert_eq!(
        store.take_answer(&old_ask2).unwrap().unwrap_err().code,
        CODE_INVALID_RESULT
    );
    assert_eq!(store.take_answer(&new_ask).unwrap(), Ok(json!(7)));
}

#[test]
fn after_a_restart_a_host_goes_on_as_its_registration_and_its_calls_keep_their_definition() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let ask = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.ask", json!(["Tidy?", null]), "k", None),
            Millis(1),
        )
        .unwrap();
    hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(2))
        .unwrap();
    // A new hub over the same store: the daemon started again.
    let (mut hub, woken) = self::hub(HubLimits {
        definitions: 0,
        ..HubLimits::default()
    });
    assert_eq!(
        hub.serve(&mut store, "desk", &r1, Vec::new(), Millis(3)),
        Err(ServeError::UnknownRegistration)
    );
    // Told so, the host registers again with its ID; its definition
    // has changed since.
    hub.register(
        &mut store,
        "desk",
        &again(1, &changed_dts()),
        &rid(2),
        Millis(4),
    )
    .unwrap();
    assert_eq!(hub.held().definitions, 1);
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(5))
        .unwrap();
    assert_eq!(ids(&sent), [ask.as_str()]);
    // The answer is checked by the definition the call was made under,
    // which the store kept: a string, though `ask` answers a number
    // now. The hub keeps no definition it has no registration of
    // (`definitions: 0`), and reads it again each time.
    hub.serve(
        &mut store,
        "desk",
        &r1,
        vec![result(&ask, json!("yes"))],
        Millis(6),
    )
    .unwrap();
    assert_eq!(store.take_answer(&ask).unwrap(), Ok(json!("yes")));
    assert_eq!(hub.held().definitions, 1);
    // An end recorded for it while no hub held it was kept for it too.
    let (mut hub, _) = self::hub(HubLimits::default());
    let end = hub
        .ended(&mut store, &r1, "run-1", End::Finished {}, None, Millis(7))
        .unwrap();
    hub.register(
        &mut store,
        "desk",
        &again(1, &desk_dts()),
        &rid(3),
        Millis(8),
    )
    .unwrap();
    let sent = hub
        .serve(&mut store, "desk", &r1, Vec::new(), Millis(9))
        .unwrap();
    assert_eq!(ids(&sent), [end.as_str()]);
    assert!(woken.lock().unwrap().is_empty());
}

/// A store that cannot be read or written.
struct Broken;

const DISK_FULL: &str = "disk full";

impl Pending for Broken {
    type Error = &'static str;
    fn issue_registration(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: Millis,
    ) -> Result<bool, Self::Error> {
        Err(DISK_FULL)
    }
    fn registration_name(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
        Err(DISK_FULL)
    }
    fn keep_definition(&mut self, _: &str, _: &str, _: Millis) -> Result<(), Self::Error> {
        Err(DISK_FULL)
    }
    fn definition_text(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
        Err(DISK_FULL)
    }
    fn keep_prompt(&mut self, _: &str, _: &str, _: Millis) -> Result<(), Self::Error> {
        Err(DISK_FULL)
    }
    fn prompt_text(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
        Err(DISK_FULL)
    }
    fn open_call(&mut self, _: NewCall, _: usize, _: Millis) -> Result<OpenedCall, Self::Error> {
        Err(DISK_FULL)
    }
    fn pending_for(&mut self, _: &str) -> Result<PendingSummary, Self::Error> {
        Err(DISK_FULL)
    }
    fn calls_for(&mut self, _: &str, _: &[String]) -> Result<Vec<PendingCall>, Self::Error> {
        Err(DISK_FULL)
    }
    fn sent_calls(&mut self, _: &[String]) -> Result<(), Self::Error> {
        Err(DISK_FULL)
    }
    fn accept_call(&mut self, _: &str, _: &str, _: Millis) -> Result<bool, Self::Error> {
        Err(DISK_FULL)
    }
    fn answer_call(
        &mut self,
        _: &str,
        _: &str,
        _: Result<Value, CallFailure>,
        _: Millis,
    ) -> Result<bool, Self::Error> {
        Err(DISK_FULL)
    }
    fn expire_calls(
        &mut self,
        _: Millis,
        _: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error> {
        Err(DISK_FULL)
    }
    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
        Err(DISK_FULL)
    }
    fn record_end(&mut self, _: NewEnd, _: usize, _: Millis) -> Result<RecordedEnd, Self::Error> {
        Err(DISK_FULL)
    }
    fn ends_for(&mut self, _: &str, _: &[String]) -> Result<Vec<PendingEnd>, Self::Error> {
        Err(DISK_FULL)
    }
    fn acknowledge_end(&mut self, _: &str, _: &str, _: Millis) -> Result<bool, Self::Error> {
        Err(DISK_FULL)
    }
    fn expire_ends(&mut self, _: Millis) -> Result<usize, Self::Error> {
        Err(DISK_FULL)
    }
}

#[test]
fn a_store_that_fails_is_an_error_of_each_method_and_leaves_no_serve_open() {
    let (mut hub, mut good, woken) = desk(HubLimits::default());
    let now = Millis(1);
    let r1 = rid(1);
    let failed = ServeError::Storage(DISK_FULL.into());
    assert_eq!(failed.body().code, wire::CODE_STORAGE_FAILED);
    assert_eq!(wire::status(&failed.body().code), 500);
    assert_eq!(
        hub.open(&mut Broken, "desk", &r1, 0, Vec::new(), now),
        Err(failed.clone())
    );
    assert_eq!(hub.held().serves, 0);
    let opened = hub
        .open(&mut good, "desk", &r1, 12_000, Vec::new(), now)
        .unwrap();
    assert_eq!(
        hub.ready(&mut Broken, &opened.serve, now),
        Err(failed.clone())
    );
    assert_eq!(hub.close(&mut Broken, opened.serve, now), Err(failed));
    assert_eq!(hub.held().serves, 0, "the serve is closed all the same");
    let refused = hub
        .enqueue(
            &mut Broken,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            now,
        )
        .unwrap_err();
    assert_eq!(refused, EnqueueError::Storage(DISK_FULL.into()));
    assert_eq!(refused.failure().code, CODE_UNAVAILABLE);
    assert_eq!(
        hub.ended(&mut Broken, &r1, "run-1", End::Finished {}, None, now),
        Err(EndedError::Storage(DISK_FULL.into()))
    );
    assert_eq!(hub.expire(&mut Broken, now), Err(DISK_FULL.to_string()));
    assert_eq!(hub.next_wake(&mut Broken, now), Err(DISK_FULL.to_string()));
    // Registering: a new one, and going on as one the hub has dropped.
    let refused = hub
        .register(&mut Broken, "desk", &request(&desk_dts()), &rid(2), now)
        .unwrap_err();
    assert_eq!(refused, RegisterError::Storage(DISK_FULL.into()));
    assert_eq!(refused.body().code, wire::CODE_STORAGE_FAILED);
    assert_eq!(
        hub.register(&mut Broken, "desk", &again(5, &desk_dts()), &rid(2), now),
        Err(RegisterError::Storage(DISK_FULL.into()))
    );
    assert_eq!(hub.held().registrations, 1);
    // An answer whose call's definition cannot be read is not taken.
    let id = hub
        .enqueue(
            &mut good,
            &r1,
            call("desk.lamps", json!([]), "k", None),
            now,
        )
        .unwrap();
    let (mut fresh, _) = self::hub(HubLimits::default());
    fresh
        .register(&mut good, "desk", &again(1, &changed_dts()), &rid(3), now)
        .unwrap();
    struct NoDefinitions(MemoryPending);
    impl Pending for NoDefinitions {
        type Error = std::convert::Infallible;
        fn issue_registration(
            &mut self,
            a: &str,
            b: &str,
            c: &str,
            d: Millis,
        ) -> Result<bool, Self::Error> {
            self.0.issue_registration(a, b, c, d)
        }
        fn registration_name(&mut self, a: &str) -> Result<Option<String>, Self::Error> {
            self.0.registration_name(a)
        }
        fn keep_definition(&mut self, a: &str, b: &str, c: Millis) -> Result<(), Self::Error> {
            self.0.keep_definition(a, b, c)
        }
        fn definition_text(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
            Ok(None)
        }
        fn keep_prompt(&mut self, a: &str, b: &str, c: Millis) -> Result<(), Self::Error> {
            self.0.keep_prompt(a, b, c)
        }
        fn prompt_text(&mut self, a: &str) -> Result<Option<String>, Self::Error> {
            self.0.prompt_text(a)
        }
        fn open_call(
            &mut self,
            a: NewCall,
            b: usize,
            c: Millis,
        ) -> Result<OpenedCall, Self::Error> {
            self.0.open_call(a, b, c)
        }
        fn pending_for(&mut self, a: &str) -> Result<PendingSummary, Self::Error> {
            self.0.pending_for(a)
        }
        fn calls_for(&mut self, a: &str, b: &[String]) -> Result<Vec<PendingCall>, Self::Error> {
            self.0.calls_for(a, b)
        }
        fn sent_calls(&mut self, a: &[String]) -> Result<(), Self::Error> {
            self.0.sent_calls(a)
        }
        fn accept_call(&mut self, a: &str, b: &str, c: Millis) -> Result<bool, Self::Error> {
            self.0.accept_call(a, b, c)
        }
        fn answer_call(
            &mut self,
            a: &str,
            b: &str,
            c: Result<Value, CallFailure>,
            d: Millis,
        ) -> Result<bool, Self::Error> {
            self.0.answer_call(a, b, c, d)
        }
        fn expire_calls(
            &mut self,
            a: Millis,
            b: &dyn Fn(&CallSummary) -> CallFailure,
        ) -> Result<Vec<String>, Self::Error> {
            self.0.expire_calls(a, b)
        }
        fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
            self.0.next_deadline()
        }
        fn record_end(
            &mut self,
            a: NewEnd,
            b: usize,
            c: Millis,
        ) -> Result<RecordedEnd, Self::Error> {
            self.0.record_end(a, b, c)
        }
        fn ends_for(&mut self, a: &str, b: &[String]) -> Result<Vec<PendingEnd>, Self::Error> {
            self.0.ends_for(a, b)
        }
        fn acknowledge_end(&mut self, a: &str, b: &str, c: Millis) -> Result<bool, Self::Error> {
            self.0.acknowledge_end(a, b, c)
        }
        fn expire_ends(&mut self, a: Millis) -> Result<usize, Self::Error> {
            self.0.expire_ends(a)
        }
    }
    let mut lossy = NoDefinitions(good);
    let refused = fresh
        .serve(&mut lossy, "desk", &r1, vec![result(&id, json!([]))], now)
        .unwrap_err();
    assert!(
        matches!(&refused, ServeError::Storage(m) if m.contains("not kept")),
        "{refused:?}"
    );
    assert!(
        lossy.0.is_open(&id),
        "the answer was not taken, and can be sent again"
    );
    assert!(woken.lock().unwrap().len() == 1);
}

// ---------------------------------------------------------------------------
// Generated sequences
// ---------------------------------------------------------------------------

/// A generator of numbers that gives the same ones each run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, of: &'a [T]) -> Option<&'a T> {
        if of.is_empty() {
            None
        } else {
            of.get(self.below(of.len() as u64) as usize)
        }
    }
}

const NAMES: [&str; 2] = ["desk", "shelf"];

/// The operations of the generated surfaces below their surface's
/// name, each with what a call of it is and the codes it declares.
const OPS: [(&str, CallKind, &[&str]); 4] = [
    ("lamps", CallKind::Read, &[]),
    ("settings.set", CallKind::Write, &["invalid"]),
    ("notify", CallKind::Effect, &[]),
    ("ask", CallKind::Effect, &["declined"]),
];

/// What the model knows of a registration there ever was.
#[derive(Debug)]
struct Registered2 {
    /// Its surface: an index into [`NAMES`].
    name: usize,
    /// Whether it last registered the changed definition, in which
    /// `ask` answers a number.
    changed: bool,
    /// When it was last registered or gone on with, among all.
    made: u64,
}

/// What the model knows of a call the hub took.
#[derive(Debug)]
struct Known {
    registration: String,
    run: String,
    /// Its operation: an index into [`OPS`].
    op: usize,
    /// Whether it was made under the changed definition.
    changed: bool,
    args: Vec<Value>,
    key: String,
    deadline: u64,
    sent: bool,
    accepted: bool,
    later: Option<u64>,
    /// What its host answered, in time: a value, or an error's code.
    answered: Option<Result<Value, String>>,
}

impl Known {
    fn open_at(&self, now: u64) -> bool {
        self.answered.is_none() && self.deadline > now
    }
}

/// The result a host answers call `id` of `op` with, which is of the
/// operation's type under the definition the call was made under, and
/// names the call wherever the type lets it.
fn good_result(op: usize, changed: bool, id: &str) -> Value {
    match (OPS[op].0, changed) {
        ("lamps", _) => json!([{"id": id, "on": true}]),
        ("ask", false) => json!(id),
        ("ask", true) => json!(
            id[1..]
                .parse::<u64>()
                .expect("the store's IDs are numbered")
        ),
        _ => Value::Null,
    }
}

/// The generated sequences' model of the hub and its store.
struct Model {
    registrations: BTreeMap<String, Registered2>,
    calls: BTreeMap<String, Known>,
    /// Each unacknowledged end's registration.
    ends: BTreeMap<String, String>,
    made: u64,
    delivered: u64,
    answered: u64,
}

impl Model {
    /// Forgets what the hosts of registrations the hub has dropped
    /// asked for later, as the hub has.
    fn forget_dropped(&mut self, hub: &Hub) {
        for known in self.calls.values_mut() {
            if hub.registration(&known.registration).is_none() {
                known.later = None;
            }
        }
    }

    /// What the answers of a serve of `serving` at `now` do, in their
    /// order.
    fn take_answers(&mut self, serving: &str, answers: &[Answer], now: u64) {
        for answer in answers {
            if self.ends.get(&answer.id).map(String::as_str) == Some(serving) {
                self.ends.remove(&answer.id);
                continue;
            }
            let Some(known) = self.calls.get_mut(&answer.id) else {
                continue;
            };
            // Its own registration's, open: an earlier answer of this
            // request that settled it has closed it.
            if known.registration != serving || !known.open_at(now) {
                continue;
            }
            match &answer.outcome {
                Outcome::Accepted => {
                    known.sent = true;
                    known.accepted = true;
                }
                Outcome::RetryAfter(ms) => known.later = Some(now + ms),
                Outcome::Result(v) => {
                    self.answered += 1;
                    known.answered =
                        Some(if *v == good_result(known.op, known.changed, &answer.id) {
                            Ok(v.clone())
                        } else {
                            Err(CODE_INVALID_RESULT.to_string())
                        });
                }
                Outcome::Error(e) => {
                    let declared = OPS[known.op].2.contains(&e.code.as_str());
                    known.answered = Some(Err(if declared || e.code == CODE_UNAVAILABLE {
                        e.code.clone()
                    } else {
                        CODE_INVALID_RESULT.to_string()
                    }));
                }
            }
        }
    }

    /// Holds what a serve of `serving` was answered with at `now` to
    /// what is due: exactly the registration's open calls that are not
    /// accepted and not left for later, and its unacknowledged ends,
    /// each once and as it was made.
    fn sent(&mut self, serving: &str, sent: &[Delivery], now: u64) {
        let name = NAMES[self.registrations[serving].name];
        let mut due: BTreeSet<&str> = self
            .calls
            .iter()
            .filter(|(_, k)| {
                k.registration == serving
                    && k.open_at(now)
                    && !k.accepted
                    && k.later.is_none_or(|t| t <= now)
            })
            .map(|(id, _)| id.as_str())
            .collect();
        due.extend(
            self.ends
                .iter()
                .filter(|(_, r)| *r == serving)
                .map(|(id, _)| id.as_str()),
        );
        assert_eq!(
            ids(sent).into_iter().collect::<BTreeSet<_>>(),
            due,
            "{serving} at {now}"
        );
        assert_eq!(sent.len(), due.len(), "nothing is sent twice in one serve");
        for delivery in sent {
            let Delivery::Call(c) = delivery else {
                continue;
            };
            self.delivered += 1;
            let known = self.calls.get_mut(&c.id).unwrap();
            let (op, kind, _) = OPS[known.op];
            assert_eq!(
                (c.op.as_str(), c.call, &c.args, &c.key, &c.run),
                (op, kind, &known.args, &known.key, &known.run),
                "a call of {name}"
            );
            // Never at or after its deadline, and with what is left of
            // it.
            assert_eq!(c.expires_in_ms, Some(known.deadline - now));
            assert_ne!(c.expires_in_ms, Some(0));
            known.sent = true;
            known.later = None;
        }
    }

    /// Reads call `id`'s outcome at `now` (the store is at `now`) and
    /// holds it to the model: the answer its own host gave it in time,
    /// else its deadline's error, else nothing yet.
    fn read_outcome(&mut self, store: &mut MemoryPending, id: &str, now: u64) {
        let known = &self.calls[id];
        let outcome = store.take_answer(id);
        match &known.answered {
            Some(want) => {
                let got = outcome.unwrap_or_else(|| panic!("{id} was answered and has no outcome"));
                assert_eq!(&got.map_err(|f| f.code), want, "{id}");
            }
            None if known.deadline <= now => {
                let want = match (known.sent, OPS[known.op].1) {
                    (false, _) => CODE_UNAVAILABLE,
                    (true, CallKind::Read) => CODE_EXPIRED,
                    (true, _) => CODE_UNKNOWN_OUTCOME,
                };
                let got = outcome
                    .unwrap_or_else(|| panic!("{id} is past its deadline and has no outcome"));
                assert_eq!(got.map_err(|f| f.code), Err(want.to_string()), "{id}");
            }
            None => {
                assert_eq!(outcome, None, "{id} was settled by no one");
                assert!(store.is_open(id), "{id} is lost");
                return;
            }
        }
        self.calls.remove(id);
    }
}

/// Everything hosts, runs and the clock can do, in an order drawn from
/// `seed`: registrations of two surfaces in two definitions, made new
/// and gone on with; calls of two runs; serves answered at once and
/// serves held open beside others; a clock that at times is given as
/// earlier than it was. Every answer and every delivery is held to a
/// model of what the module's documentation says holds. Answers how
/// many calls were sent and how many answered with a result.
fn generated(seed: u64, steps: usize) -> (u64, u64) {
    let limits = HubLimits {
        registrations: 4,
        registrations_per_name: 3,
        calls_per_registration: 6,
        ended_per_registration: 3,
        args_bytes: 120,
        result_bytes: 120,
        answers_per_serve: 5,
        serves_per_registration: 2,
        ..HubLimits::default()
    };
    let (mut hub, _) = hub(limits.clone());
    let mut store = MemoryPending::new();
    let mut rng = Rng(seed | 1);
    // The definition of each surface, as it is and changed.
    let dts: Vec<[String; 2]> = NAMES
        .iter()
        .map(|name| {
            [desk_dts(), changed_with_lamps()].map(|text| {
                text.replace("declare const desk", &format!("declare const {name}"))
                    .replace("@allow desk.*", &format!("@allow {name}.*"))
            })
        })
        .collect();
    let mut model = Model {
        registrations: BTreeMap::new(),
        calls: BTreeMap::new(),
        ends: BTreeMap::new(),
        made: 0,
        delivered: 0,
        answered: 0,
    };
    let mut now = 0u64;
    let mut drawn = 0u64;
    let mut keys = 0u64;
    let mut open: Vec<Serve> = Vec::new();

    for step in 0..steps {
        // The clock, and what it alone did.
        now += match rng.below(256) {
            0..=159 => 0,
            160..=249 => rng.below(2_000),
            250..=254 => rng.below(40_000),
            _ => rng.below(700_000),
        };
        hub.expire(&mut store, Millis(now)).unwrap();
        model.forget_dropped(&hub);
        // What each method is given: now, or an instant before it,
        // which is read as now.
        let at = Millis(if rng.below(8) == 0 {
            now.saturating_sub(rng.below(50_000))
        } else {
            now
        });
        let all: Vec<String> = model.registrations.keys().cloned().collect();
        let live: Vec<String> = all
            .iter()
            .filter(|id| hub.registration(id).is_some())
            .cloned()
            .collect();
        // Mostly the registrations the hub holds, and sometimes one it
        // has dropped.
        let pool = if rng.below(8) == 0 { &all } else { &live };

        match if live.is_empty() { 0 } else { rng.below(120) } {
            // A host registers: a new registration, one it was, one of
            // another surface, or one that never was.
            0..=2 => {
                let name = rng.below(2) as usize;
                let changed = rng.below(3) == 0;
                let named = match rng.below(8) {
                    0..=2 => rng.pick(&all).cloned(),
                    3 => Some(rid(900 + rng.below(3))),
                    _ => None,
                };
                drawn += 1;
                let fresh = rid(drawn);
                let asked = RegisterRequest {
                    registration: named.clone(),
                    ..request(&dts[name][usize::from(changed)])
                };
                let is_held = named
                    .as_ref()
                    .is_some_and(|n| hub.registration(n).is_some());
                let issued = named
                    .as_ref()
                    .map(|n| model.registrations.get(n).map(|r| r.name));
                // Which registrations must go to make room, by the
                // rule: the oldest unconnected of the name, then of
                // all.
                let oldest = |among: Vec<&String>, hub: &mut Hub| {
                    among
                        .into_iter()
                        .filter(|id| !hub.connected(id, at))
                        .min_by_key(|id| model.registrations[*id].made)
                        .cloned()
                };
                let of_name: Vec<&String> = live
                    .iter()
                    .filter(|id| model.registrations[*id].name == name)
                    .collect();
                let mut room: Result<Vec<String>, bool> = Ok(Vec::new());
                if !is_held {
                    if of_name.len() >= limits.registrations_per_name {
                        room = oldest(of_name, &mut hub).map(|id| vec![id]).ok_or(true);
                    }
                    if let Ok(gone) = &mut room
                        && live.len() - gone.len() >= limits.registrations
                    {
                        let rest = live.iter().filter(|id| !gone.contains(id)).collect();
                        match oldest(rest, &mut hub) {
                            Some(id) => gone.push(id),
                            None => room = Err(false),
                        }
                    }
                }
                let registered = hub.register(&mut store, NAMES[name], &asked, &fresh, at);
                match (issued, room) {
                    (Some(issued), _) if issued != Some(name) => {
                        assert_eq!(registered, Err(RegisterError::UnknownRegistration));
                    }
                    (_, Err(of_name)) => {
                        let limit = if of_name { 3 } else { 4 };
                        assert_eq!(
                            registered,
                            Err(RegisterError::TooManyRegistrations { limit, of_name })
                        );
                    }
                    (_, Ok(gone)) => {
                        let registered = registered.unwrap();
                        let id = named.unwrap_or(fresh);
                        assert_eq!(registered.registration, id);
                        for id in &gone {
                            assert!(hub.registration(id).is_none(), "{id} did not make room");
                        }
                        for id in live.iter().filter(|id| !gone.contains(id)) {
                            assert!(hub.registration(id).is_some(), "{id} was dropped");
                        }
                        open.retain(|serve| !gone.iter().any(|id| id == serve.registration()));
                        model.made += 1;
                        let made = model.made;
                        model.registrations.insert(
                            id,
                            Registered2 {
                                name,
                                changed,
                                made,
                            },
                        );
                        model.forget_dropped(&hub);
                    }
                }
            }
            // A run makes a call.
            3..=36 => {
                let registration = match rng.below(8) {
                    0 => rid(999),
                    _ => rng.pick(pool).cloned().unwrap_or_default(),
                };
                let of = model.registrations.get(&registration);
                let op = rng.below(OPS.len() as u64) as usize;
                // Of its registration's surface, and sometimes of the
                // other.
                let name = of.map_or(0, |r| r.name) ^ usize::from(rng.below(16) == 0);
                let args = match OPS[op].0 {
                    "lamps" => vec![],
                    "settings.set" => vec![json!({"theme": "dark"})],
                    "notify" => vec![json!("n".repeat(rng.below(160) as usize))],
                    _ => vec![json!("Tidy?"), Value::Null],
                };
                let run = format!("run-{}", 1 + rng.below(2));
                // A key of its own, or one the model has seen: of this
                // run or of the other.
                let old: Vec<String> = model.calls.values().map(|k| k.key.clone()).collect();
                let reuses = rng.below(5) == 0;
                let key = match rng.pick(&old) {
                    Some(key) if reuses => key.clone(),
                    _ => {
                        keys += 1;
                        format!("{run}:{keys}:0")
                    }
                };
                let given = match rng.below(12) {
                    0 => None,
                    1 => Some(now.saturating_sub(rng.below(3))),
                    2 => Some(now + 1 + rng.below(60_000)),
                    _ => Some(now + 1 + rng.below(6_000)),
                };
                let deadline = given.unwrap_or(now + limits.call_ms);
                let connected = hub.connected(&registration, at);
                let queued = model
                    .calls
                    .values()
                    .filter(|k| k.registration == registration && k.open_at(now))
                    .count();
                let keyed = model
                    .calls
                    .iter()
                    .find(|(_, k)| k.key == key)
                    .map(|(id, _)| id.clone());
                let takes = of.is_some_and(|r| r.name == name)
                    && hub.registration(&registration).is_some()
                    && connected
                    && deadline > now
                    && json_bytes(&args) <= limits.args_bytes;
                let changed = of.is_some_and(|r| r.changed);
                let before = store.held();
                let made = hub.enqueue(
                    &mut store,
                    &registration,
                    Enqueue {
                        run: run.clone(),
                        op: format!("{}.{}", NAMES[name], OPS[op].0),
                        args: args.clone(),
                        key: key.clone(),
                        deadline: given.map(Millis),
                    },
                    at,
                );
                match (made, keyed) {
                    (Ok(id), Some(held)) => {
                        // The call the store holds under the key: the
                        // same in everything.
                        let known = &model.calls[&held];
                        assert_eq!(id, held);
                        assert_eq!(
                            (&known.registration, &known.run, known.op, &known.args),
                            (&registration, &run, op, &args)
                        );
                        assert!(takes);
                        assert_eq!(store.held(), before);
                    }
                    (Ok(id), None) => {
                        assert!(takes && queued < limits.calls_per_registration, "{id}");
                        assert!(!model.calls.contains_key(&id), "{id} names two calls");
                        model.calls.insert(
                            id,
                            Known {
                                registration,
                                run,
                                op,
                                changed,
                                args,
                                key,
                                deadline,
                                sent: false,
                                accepted: false,
                                later: None,
                                answered: None,
                            },
                        );
                    }
                    (Err(e), keyed) => {
                        assert_eq!(store.held(), before, "{e:?} left something");
                        let same = keyed.as_ref().is_some_and(|held| {
                            let known = &model.calls[held];
                            (&known.registration, &known.run, known.op, &known.args)
                                == (&registration, &run, op, &args)
                        });
                        assert!(
                            !takes
                                || (keyed.is_some() && !same)
                                || (keyed.is_none() && queued >= limits.calls_per_registration),
                            "{e:?} refused a call that should have been taken"
                        );
                    }
                }
            }
            // A host serves, with answers: at once, or leaving the
            // serve open.
            37..=94 => {
                let named = match rng.below(8) {
                    0 => rid(999),
                    _ => rng.pick(pool).cloned().unwrap_or_default(),
                };
                let of = model.registrations.get(&named);
                // Under its surface's name, and sometimes the other's.
                let name = of.map_or(0, |r| r.name) ^ usize::from(rng.below(12) == 0);
                let serving =
                    hub.registration(&named).is_some() && of.is_some_and(|r| r.name == name);
                let known: Vec<String> = model
                    .calls
                    .keys()
                    .chain(model.ends.keys())
                    .cloned()
                    .collect();
                // Mostly its own open calls, and sometimes anything.
                let own: Vec<String> = model
                    .calls
                    .iter()
                    .filter(|(_, k)| k.registration == named && k.open_at(now))
                    .map(|(id, _)| id.clone())
                    .collect();
                let mut answers = Vec::new();
                for _ in 0..rng.below(7) {
                    let from = match rng.below(8) {
                        0 => &[][..],
                        1..=2 => &known[..],
                        _ => &own[..],
                    };
                    let id = match rng.pick(from) {
                        Some(id) => id.clone(),
                        None => format!("c{}", rng.below(50)),
                    };
                    let of = model.calls.get(&id);
                    let (op, changed) = of.map_or((0, false), |k| (k.op, k.changed));
                    let code = [
                        "declined",
                        "invalid",
                        "unavailable",
                        "expired",
                        "Not A Code",
                    ];
                    let outcome = match rng.below(14) {
                        0..=5 => Outcome::Result(good_result(op, changed, &id)),
                        // What the other definition would take.
                        6 => Outcome::Result(good_result(op, !changed, &id)),
                        7 => Outcome::Result(json!({"wrong": "x".repeat(rng.below(200) as usize)})),
                        8..=9 => Outcome::Error(CallError {
                            code: code[rng.below(5) as usize].into(),
                            message: "no".into(),
                        }),
                        10..=11 => Outcome::Accepted,
                        _ => Outcome::RetryAfter(rng.below(5_000)),
                    };
                    answers.push(Answer { id, outcome });
                }
                let before = store.held();
                let opened = hub.open(&mut store, NAMES[name], &named, 12_000, answers.clone(), at);
                if !serving {
                    assert_eq!(opened, Err(ServeError::UnknownRegistration));
                    assert_eq!(store.held(), before);
                    continue;
                }
                if answers.len() > limits.answers_per_serve {
                    assert_eq!(opened, Err(ServeError::TooManyAnswers { limit: 5 }));
                    assert_eq!(store.held(), before);
                    continue;
                }
                let opened = opened.unwrap();
                model.take_answers(&named, &answers, now);
                assert_eq!(opened.serve.until(), Millis(now + 12_000));
                if rng.below(6) == 0 {
                    open.push(opened.serve);
                } else {
                    // The newest of its registration's serves is never
                    // the one that was ended.
                    let sent = hub.close(&mut store, opened.serve, at).unwrap();
                    model.sent(&named, &sent, now);
                }
            }
            // A serve that was left open is closed.
            95..=100 => {
                if open.is_empty() {
                    continue;
                }
                let serve = open.remove(rng.below(open.len() as u64) as usize);
                let named = serve.registration().to_string();
                if rng.below(4) == 0 {
                    // Its request went away: nothing is sent, and
                    // nothing marked as sent.
                    hub.abandon(serve, at);
                    continue;
                }
                let ready = hub.ready(&mut store, &serve, at).unwrap();
                let sent = hub.close(&mut store, serve, at).unwrap();
                if ready == Ready::Ended {
                    assert_eq!(sent, [], "a serve that ended is answered with nothing");
                } else {
                    let new = sent.iter().any(|d| match d {
                        Delivery::Call(c) => {
                            !model.calls[&c.id].sent || model.calls[&c.id].later.is_some()
                        }
                        _ => false,
                    });
                    assert!(
                        ready == Ready::Deliver || !new,
                        "something new did not make it ready"
                    );
                    model.sent(&named, &sent, now);
                }
            }
            // A run reads its call's outcome.
            101..=112 => {
                let known: Vec<String> = model.calls.keys().cloned().collect();
                if let Some(id) = rng.pick(&known) {
                    model.read_outcome(&mut store, id, now);
                }
            }
            // A run ends.
            _ => {
                let registration = rng.pick(pool).cloned().unwrap_or_default();
                let run = format!("run-{}", rng.below(6));
                let queued = model.ends.values().filter(|r| **r == registration).count();
                match hub.ended(&mut store, &registration, &run, End::Finished {}, None, at) {
                    Ok(id) => {
                        assert!(
                            model.ends.contains_key(&id) || queued < limits.ended_per_registration
                        );
                        model.ends.insert(id, registration);
                    }
                    Err(e) => {
                        assert_eq!(e, EndedError::QueueFull { limit: 3 });
                        assert!(queued >= limits.ended_per_registration);
                    }
                }
            }
        }

        // Nothing is past its bound, and the store holds what the model
        // does. (Every registration there ever was is looked at now
        // and then, and the ones the hub holds each time.)
        let held = hub.held();
        assert!(held.registrations <= limits.registrations);
        for name in 0..2 {
            let of_name = model
                .registrations
                .iter()
                .filter(|(id, r)| r.name == name && hub.registration(id).is_some())
                .count();
            assert!(of_name <= limits.registrations_per_name);
        }
        assert!(held.serves <= held.registrations * limits.serves_per_registration);
        assert!(held.deferred <= held.registrations * limits.calls_per_registration);
        assert!(held.definitions <= held.registrations + limits.definitions);
        let looked_at = if step % 64 == 0 { &all } else { &live };
        for id in looked_at {
            let pending = store.pending_for(id).unwrap();
            assert!(pending.calls.len() <= limits.calls_per_registration, "{id}");
            assert!(pending.ends.len() <= limits.ended_per_registration, "{id}");
        }
        assert_eq!(store.held(), (model.calls.len(), model.ends.len()));
    }

    // No call was lost: every one the hub took has an outcome once its
    // deadline is past, and the ones given none still have their day.
    now += 200_000;
    hub.expire(&mut store, Millis(now)).unwrap();
    let all: Vec<String> = model.calls.keys().cloned().collect();
    for id in all {
        model.read_outcome(&mut store, &id, now);
    }
    assert!(
        model
            .calls
            .values()
            .all(|k| k.deadline > now && k.answered.is_none())
    );
    assert_eq!(store.held().0, model.calls.len());
    (model.delivered, model.answered)
}

/// The fixture's surface with `ask` answering a number, and nothing
/// else changed.
fn changed_with_lamps() -> String {
    desk_dts().replace(
        "choices?: string[]): Promise<string>",
        "choices?: string[]): Promise<Int>",
    )
}

#[test]
fn no_sequence_of_serves_calls_answers_and_clock_breaks_what_holds() {
    let (mut delivered, mut answered) = (0, 0);
    for seed in 1..=16u64 {
        let (d, a) = generated(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 2_500);
        delivered += d;
        answered += a;
    }
    assert!(
        delivered > 1_000 && answered > 500,
        "{delivered} sent, {answered} answered"
    );
}

#[test]
fn answers_of_any_json_settle_as_a_value_of_the_type_or_as_invalid_result() {
    fn any(rng: &mut Rng, depth: u32) -> Value {
        match rng.below(if depth == 0 { 5 } else { 7 }) {
            0 => Value::Null,
            1 => json!(rng.below(2) == 0),
            2 => json!(rng.below(5)),
            3 => json!(["id", "on", "l1", ""][rng.below(4) as usize]),
            4 => json!(rng.next() as f64 / 3.0),
            5 => (0..rng.below(3)).map(|_| any(rng, depth - 1)).collect(),
            _ => (0..rng.below(4))
                .map(|_| {
                    let key = ["id", "on", "x"][rng.below(3) as usize];
                    (key.to_string(), any(rng, depth - 1))
                })
                .collect::<serde_json::Map<_, _>>()
                .into(),
        }
    }
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let mut rng = Rng(7);
    let (mut values, mut refused) = (0, 0);
    for n in 0..3_000u64 {
        let at = Millis(n);
        let op = format!("desk.{}", OPS[rng.below(4) as usize].0);
        let args = match op.as_str() {
            "desk.lamps" => json!([]),
            "desk.settings.set" => json!([{}]),
            "desk.notify" => json!(["hi"]),
            _ => json!(["Tidy?", null]),
        };
        let id = hub
            .enqueue(&mut store, &r1, call(&op, args, &format!("k{n}"), None), at)
            .unwrap();
        let value = any(&mut rng, 3);
        hub.serve(
            &mut store,
            "desk",
            &r1,
            vec![result(&id, value.clone())],
            at,
        )
        .unwrap();
        let registration = hub.registration(&r1).unwrap();
        let operation = registration.definition.operation(&op).unwrap();
        match store.take_answer(&id).expect("an answer settles its call") {
            Ok(got) => {
                assert_eq!(got, value);
                operation.check_answer(&value).unwrap();
                values += 1;
            }
            Err(failure) => {
                assert_eq!(failure.code, CODE_INVALID_RESULT, "{value}");
                operation.check_answer(&value).unwrap_err();
                refused += 1;
            }
        }
    }
    assert!(
        values > 100 && refused > 100,
        "{values} values, {refused} refused"
    );
}

// ---------------------------------------------------------------------------
// What is read and judged with the hub let go, and what a host is held to
// ---------------------------------------------------------------------------

#[test]
fn a_request_read_first_and_registered_after_is_the_registration_register_makes() {
    let limits = HubLimits::default();
    let (mut hub, mut store, _) = desk(limits.clone());
    let asked = RegisterRequest {
        prompt: None,
        settings: Settings {
            allow: Some(vec!["desk.*".into()]),
            ..Settings::default()
        },
        ..request(&desk_dts())
    };
    let reading = read_registration(&limits, hub.reserved(), "desk", &asked).unwrap();
    assert_eq!(reading.hash(), hub.registration(&rid(1)).unwrap().hash);
    assert_eq!(reading.settings(), &asked.settings);
    assert_eq!(reading.prompt(), None);
    let registered = hub
        .register_parsed(&mut store, reading, &rid(2), Millis(1))
        .unwrap();
    let whole = hub
        .register(&mut store, "desk", &asked, &rid(3), Millis(1))
        .unwrap();
    assert_eq!(
        (&registered.hash, &registered.withheld),
        (&whole.hash, &whole.withheld)
    );
    let (a, b) = (
        hub.snapshot(&rid(2)).unwrap(),
        hub.snapshot(&rid(3)).unwrap(),
    );
    assert_eq!(
        (&a.name, &a.hash, &a.settings),
        (&b.name, &b.hash, &b.settings)
    );
    assert!(Arc::ptr_eq(
        &a.definition,
        &hub.snapshot(&rid(2)).unwrap().definition
    ));
    assert!(hub.snapshot(&rid(9)).is_none());
    // What only reading finds is found with no hub: the version, the
    // name, the definition.
    let other = RegisterRequest {
        clax_surface: 2,
        ..asked.clone()
    };
    assert_eq!(
        read_registration(&limits, hub.reserved(), "desk", &other).unwrap_err(),
        RegisterError::UnsupportedVersion(2)
    );
    assert!(matches!(
        read_registration(&limits, hub.reserved(), "shelf", &asked),
        Err(RegisterError::NameMismatch { .. })
    ));
}

#[test]
fn answers_judged_with_the_hub_let_go_settle_as_those_judged_under_it() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let r1 = rid(1);
    let limits = hub.limits().clone();
    // Six calls, twice: one set answered through `open`, the other
    // judged first.
    let made = |hub: &mut Hub, store: &mut MemoryPending, set: &str| -> Vec<String> {
        [
            ("desk.lamps", json!([])),
            ("desk.lamps", json!([])),
            ("desk.ask", json!(["Tidy?", null])),
            ("desk.ask", json!(["Tidy?", null])),
            ("desk.notify", json!(["hi"])),
            ("desk.notify", json!(["hi"])),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (op, args))| {
            hub.enqueue(
                store,
                &rid(1),
                call(op, args, &format!("{set}{n}"), None),
                Millis(1),
            )
            .unwrap()
        })
        .collect()
    };
    let answers = |ids: &[String]| {
        vec![
            result(&ids[0], json!([{"id": "l1", "on": true}])),
            result(&ids[1], json!({"not": "lamps"})),
            error(&ids[2], "declined", "no"),
            error(&ids[3], "expired", "no"),
            Answer {
                id: ids[4].clone(),
                outcome: Outcome::Accepted,
            },
            result(&ids[4], json!(null)),
            Answer {
                id: ids[5].clone(),
                outcome: Outcome::RetryAfter(50),
            },
            result("c999", json!(1)),
        ]
    };
    let under = made(&mut hub, &mut store, "a");
    let opened = hub
        .open(&mut store, "desk", &r1, 0, answers(&under), Millis(2))
        .unwrap();
    let outside = made(&mut hub, &mut store, "b");
    let before = hub.settles();
    let asked = answers(&outside);
    let checking = hub
        .checkable(&mut store, "desk", &r1, &asked, Millis(2))
        .unwrap();
    // Only the results and errors of its open calls are to be judged.
    assert_eq!(checking.len(), 5);
    assert_eq!(hub.settles(), before, "looking settles nothing");
    let judged = check_answers(&limits, asked, &checking);
    let checked = hub
        .open_checked(&mut store, "desk", &r1, 0, judged, Millis(2))
        .unwrap();
    assert_eq!(
        (opened.ignored, opened.settled.len()),
        (checked.ignored, checked.settled.len())
    );
    assert_eq!(hub.settles(), before + 5);
    for (a, b) in under.iter().zip(&outside) {
        assert_eq!(store.outcome_of(a), store.outcome_of(b), "{a} and {b}");
    }
    assert_eq!(
        store
            .outcome_of(&outside[1])
            .unwrap()
            .as_ref()
            .unwrap_err()
            .code,
        CODE_INVALID_RESULT
    );
    assert!(store.is_open(&outside[5]));
    // A judged answer for a call that was settled meanwhile is ignored.
    let late = hub
        .enqueue(
            &mut store,
            &r1,
            call("desk.lamps", json!([]), "late", None),
            Millis(3),
        )
        .unwrap();
    let asked = vec![result(&late, json!([]))];
    let checking = hub
        .checkable(&mut store, "desk", &r1, &asked, Millis(3))
        .unwrap();
    let judged = check_answers(&limits, asked, &checking);
    hub.serve(
        &mut store,
        "desk",
        &r1,
        vec![error(&late, "unavailable", "x")],
        Millis(3),
    )
    .unwrap();
    let opened = hub
        .open_checked(&mut store, "desk", &r1, 0, judged, Millis(3))
        .unwrap();
    assert_eq!((opened.ignored, opened.settled.len()), (1, 0));
    assert_eq!(
        store.take_answer(&late).unwrap().unwrap_err().code,
        CODE_UNAVAILABLE
    );
    // What `checkable` refuses is what `open` refuses.
    assert_eq!(
        hub.checkable(&mut store, "shelf", &r1, &[], Millis(3))
            .unwrap_err(),
        ServeError::UnknownRegistration
    );
}

#[test]
fn a_registrations_settings_are_held_to_their_bounds() {
    let limits = HubLimits::default();
    let (hub, _) = hub(limits.clone());
    let with = |settings: Value| RegisterRequest {
        settings: serde_json::from_value(settings).unwrap(),
        ..request(&desk_dts())
    };
    let read =
        |settings: Value| read_registration(&limits, hub.reserved(), "desk", &with(settings));
    let refused = |settings: Value, needle: &str| {
        let e = read(settings.clone()).unwrap_err();
        assert_eq!(e.body().code, wire::CODE_INVALID_SETTINGS, "{settings}");
        assert!(e.to_string().contains(needle), "{e}");
        assert!(
            e.to_string().len() < 600,
            "a refusal is short: {}",
            e.to_string().len()
        );
    };
    let name = "a".repeat(64);
    // The longest pattern a definition's operation can be named by is
    // taken: 34 names of 64 bytes, and `.*`.
    let longest = format!("{}.*", vec![name.as_str(); 34].join("."));
    read(json!({"allow": ["*", "desk.*", "desk.settings.set", longest]})).unwrap();
    refused(
        json!({"allow": [format!("desk.{}", "a".repeat(65))]}),
        "too long",
    );
    refused(
        json!({"allow": [vec![name.as_str(); 35].join(".")]}),
        "too long",
    );
    refused(
        json!({"allow": [format!("desk.{}", "a".repeat(7 * 1024 * 1024))]}),
        "too long",
    );
    refused(json!({"allow": ["not a pattern"]}), "not an allow pattern");
    refused(json!({"allow": vec!["desk.*"; 129]}), "129 patterns");
    // All of them together.
    refused(json!({"allow": vec![longest.as_str(); 100]}), "16384");
    // Each limit: "none", or a number in its range.
    read(
        json!({"call_timeout_s": "none", "wait_limit_s": 1, "max_effects_per_run": 0,
                "runs": {"max_steps": 1_000_000, "max_run_minutes": 525_600}}),
    )
    .unwrap();
    refused(json!({"call_timeout_s": 0}), "`call_timeout_s` is 0");
    refused(json!({"wait_limit_s": 31_536_001}), "from 1 to 31536000");
    refused(json!({"wait_limit_s": u64::MAX}), "`wait_limit_s`");
    refused(
        json!({"max_effects_per_run": 1_000_001}),
        "from 0 to 1000000",
    );
    refused(json!({"runs": {"max_steps": 0}}), "`runs.max_steps` is 0");
    refused(
        json!({"runs": {"max_run_minutes": 525_601}}),
        "`runs.max_run_minutes`",
    );
    // A start's `allow` is read by the same rule.
    assert_eq!(
        allow_patterns(&["desk.*".to_string()], &limits.definition)
            .unwrap()
            .len(),
        1
    );
    allow_patterns(&["a".repeat(7 * 1024 * 1024)], &limits.definition).unwrap_err();
}

#[test]
fn a_registration_may_change_only_so_often() {
    let (mut hub, mut store, _) = desk(HubLimits::default());
    let dts = desk_dts();
    let changed = |n: u64| {
        again(
            1,
            &dts.replace("A desk and what is on it.", &format!("A desk, as of {n}.")),
        )
    };
    // Going on with nothing changed is no change, however often.
    for at in 1..=20 {
        hub.register(&mut store, "desk", &again(1, &dts), &rid(2), Millis(at))
            .unwrap();
    }
    // Eight changes in a minute, and not a ninth: of the definition,
    // the prompt file or the settings.
    for n in 1..=8 {
        hub.register(&mut store, "desk", &changed(n), &rid(2), Millis(1_000 + n))
            .unwrap();
    }
    let hash = hub.registration(&rid(1)).unwrap().hash.to_string();
    let with_settings = RegisterRequest {
        settings: Settings {
            call_timeout_s: Some(wire::Bound::At(5)),
            ..Settings::default()
        },
        ..changed(8)
    };
    for ninth in [changed(9), with_settings.clone()] {
        let refused = hub
            .register(&mut store, "desk", &ninth, &rid(2), Millis(2_000))
            .unwrap_err();
        assert_eq!(
            refused,
            RegisterError::TooManyChanges {
                limit: 8,
                window_ms: 60_000
            }
        );
        assert_eq!(refused.body().code, CODE_TOO_MANY_CHANGES);
    }
    // Refused, it is as it was, and still gone on with unchanged.
    assert_eq!(hub.registration(&rid(1)).unwrap().hash, hash);
    hub.register(&mut store, "desk", &changed(8), &rid(2), Millis(2_001))
        .unwrap();
    // The window moves: the first change was at 1,001.
    assert!(
        hub.register(&mut store, "desk", &changed(9), &rid(2), Millis(61_000))
            .is_err()
    );
    hub.register(&mut store, "desk", &with_settings, &rid(2), Millis(61_001))
        .unwrap();
    assert!(
        hub.register(&mut store, "desk", &changed(9), &rid(2), Millis(61_001))
            .is_err()
    );
    hub.register(&mut store, "desk", &changed(9), &rid(2), Millis(61_002))
        .unwrap();
    // Another registration has its own count.
    hub.register(&mut store, "desk", &request(&dts), &rid(3), Millis(61_003))
        .unwrap();
    hub.register(
        &mut store,
        "desk",
        &again(3, &changed(1).definition),
        &rid(4),
        Millis(61_004),
    )
    .unwrap();
}

#[test]
fn what_a_host_and_a_script_are_told_of_a_store_that_failed_is_not_what_the_store_said() {
    let said = "no such table: bot_pending_calls at /Users/someone/.clax/clax.db";
    let bodies = [
        ServeError::Storage(said.into()).body(),
        RegisterError::Storage(said.into()).body(),
    ];
    for body in bodies {
        assert_eq!(body.code, wire::CODE_STORAGE_FAILED);
        assert!(
            !body.message.contains("clax.db") && !body.message.contains("table"),
            "{body:?}"
        );
    }
    let failure = EnqueueError::Storage(said.into()).failure();
    assert_eq!(
        (failure.code.as_str(), failure.retryable),
        (CODE_UNAVAILABLE, true)
    );
    assert!(!failure.message.contains("clax.db"), "{failure:?}");
    // The words are kept where a log reads them.
    assert!(
        ServeError::Storage(said.into())
            .to_string()
            .contains("clax.db")
    );
}

/// A registration's prompt file is kept under its SHA-256 when it is
/// registered and when it is gone on with: the snapshot a run is
/// started from names the one the registration has now, and the one it
/// had is still kept, for the runs started with it.
#[test]
fn a_registrations_prompt_file_is_kept_by_its_hash_beside_its_definition() {
    let (mut hub, _) = hub(HubLimits::default());
    let mut store = MemoryPending::new();
    let first = fixture()["files"]["desk.prompt.md"]
        .as_str()
        .expect("the fixture has desk.prompt.md")
        .to_string();
    let second = first.replace("You look after a desk", "You keep another desk");
    assert_ne!(second, first);
    let with = |request: RegisterRequest, prompt: &str| RegisterRequest {
        prompt: Some(prompt.to_string()),
        ..request
    };
    hub.register(
        &mut store,
        "desk",
        &with(request(&desk_dts()), &first),
        &rid(1),
        Millis(0),
    )
    .unwrap();
    let first_hash = sha256_hex(first.as_bytes());
    let snapshot = hub.snapshot(&rid(1)).unwrap();
    assert_eq!(snapshot.prompt.as_deref(), Some(first.as_str()));
    assert_eq!(snapshot.prompt_hash.as_deref(), Some(first_hash.as_str()));
    assert_eq!(store.prompt_text(&first_hash).unwrap(), Some(first.clone()));
    // Gone on with another definition and another prompt file.
    hub.register(
        &mut store,
        "desk",
        &with(again(1, &changed_dts()), &second),
        &rid(2),
        Millis(1),
    )
    .unwrap();
    let second_hash = sha256_hex(second.as_bytes());
    let snapshot = hub.snapshot(&rid(1)).unwrap();
    assert_eq!(snapshot.prompt_hash.as_deref(), Some(second_hash.as_str()));
    assert_eq!(store.prompt_text(&second_hash).unwrap(), Some(second));
    assert_eq!(store.prompt_text(&first_hash).unwrap(), Some(first));
    // Gone on with none: the snapshot names none, and nothing is kept
    // for it.
    hub.register(
        &mut store,
        "desk",
        &again(1, &desk_dts()),
        &rid(3),
        Millis(2),
    )
    .unwrap();
    let snapshot = hub.snapshot(&rid(1)).unwrap();
    assert_eq!((snapshot.prompt, snapshot.prompt_hash), (None, None));
}
