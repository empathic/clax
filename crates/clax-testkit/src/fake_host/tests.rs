//! [`FakeHost`] against the run loop's hub in process ([`MemoryHub`]):
//! the contract fixture's script, and each behaviour.

use super::*;
use clax_surface::wire::{CODE_UNKNOWN_REGISTRATION, End};

const FIXTURE: &str = include_str!("../../../../docs/claxbot-surfaces/contract.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the fixture is JSON")
}

fn file(name: &str) -> String {
    fixture()["files"][name]
        .as_str()
        .expect("a file")
        .to_string()
}

/// A hub at 0 and a host of `desk` on it, registered.
async fn desk(
    host: impl FnOnce(FakeHost<MemoryHub>) -> FakeHost<MemoryHub>,
) -> (MemoryHub, FakeHost<MemoryHub>) {
    let hub = MemoryHub::new(HostClock::at(0));
    let mut host = host(
        FakeHost::new(hub.clone(), "desk", Notation::Dts, &file("desk.d.ts"))
            .with_clock(hub.clock().clone()),
    );
    host.register().await.expect("desk registers");
    (hub, host)
}

/// A run's call of `op` for the host's registration, with 30 s to
/// answer.
fn call(hub: &MemoryHub, host: &FakeHost<MemoryHub>, op: &str, args: Value, key: &str) -> String {
    let deadline = hub.clock().now() + 30_000;
    hub.call(
        host.registration().unwrap(),
        "run-1",
        &format!("desk.{op}"),
        args.as_array().unwrap().clone(),
        key,
        Some(deadline),
    )
    .expect("the call is enqueued")
}

fn delivered(served: &Served) -> usize {
    match served {
        Served::Delivered(d) => d.len(),
        other => panic!("not delivered: {other:?}"),
    }
}

#[tokio::test]
async fn the_fixtures_script_replays_clean_through_the_transport() {
    let mut hub = MemoryHub::new(HostClock::at(0));
    let names = replay(&fixture(), &hub.clone(), &mut hub).await;
    assert_eq!(names.differences(), &[] as &[String]);
    assert_eq!(
        hub.held(),
        (0, 0),
        "every call settled and read, every end acknowledged"
    );
    assert_eq!(names.get("$a"), Some(&json!(registration_id(1))));
}

#[tokio::test]
async fn a_replay_reports_what_differs() {
    // The first registration's hash, and the first refusal's status.
    let mut fixture = fixture();
    let script = fixture["script"].as_array_mut().unwrap();
    let registered = script
        .iter()
        .position(|s| s["host"] == "register" && s["status"] == 200)
        .unwrap();
    script[registered]["response"]["hash"] = json!("not the hash");
    let refused = script
        .iter()
        .position(|s| s["host"] == "serve" && s["status"] == 404)
        .unwrap();
    let status = script[refused]["status"].clone();
    script[refused]["status"] = json!(299);
    let mut hub = MemoryHub::new(HostClock::at(0));
    let names = replay(&fixture, &hub.clone(), &mut hub).await;
    let differences = names.differences();
    assert_eq!(differences.len(), 2, "{differences:#?}");
    assert!(differences[0].contains("not the hash"), "{differences:#?}");
    assert!(
        differences[1].contains(&format!("status {status}")),
        "{differences:#?}"
    );
}

#[tokio::test]
async fn either_notation_registers_and_a_second_register_goes_on_as_the_first() {
    let hub = MemoryHub::new(HostClock::at(0));
    let mut dts = FakeHost::new(hub.clone(), "desk", Notation::Dts, &file("desk.d.ts"));
    let mut manifest = FakeHost::new(hub.clone(), "desk", Notation::Manifest, &file("desk.json"));
    let a = dts.register().await.unwrap();
    let b = manifest.register().await.unwrap();
    assert_ne!(a.registration, b.registration);
    assert_eq!(a.hash, b.hash);
    assert_eq!(dts.register().await.unwrap().registration, a.registration);
    let mut wrong = FakeHost::new(hub.clone(), "shelf", Notation::Dts, &file("desk.d.ts"));
    let (status, body) = wrong.register().await.unwrap_err();
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (400, Some("name_mismatch"))
    );
}

#[tokio::test]
async fn a_result_or_an_error_is_answered_with_the_next_round() {
    let (hub, mut host) = desk(|h| {
        h.on(
            "lamps",
            Behaviour::result(json!([{"id": "l1", "on": true}])),
        )
        .on("settings.set", Behaviour::error("invalid", "no such theme"))
    })
    .await;
    let lamps = call(&hub, &host, "lamps", json!([]), "run-1:1:0");
    let set = call(
        &hub,
        &host,
        "settings.set",
        json!([{"theme": "dark"}]),
        "run-1:2:0",
    );
    assert_eq!(delivered(&host.serve_once().await), 2);
    assert_eq!(
        hub.outcome(&lamps),
        None,
        "answered with the next round, not this one"
    );
    assert_eq!(delivered(&host.serve_once().await), 0);
    assert_eq!(
        hub.outcome(&lamps),
        Some(Ok(json!([{"id": "l1", "on": true}])))
    );
    assert_eq!(hub.outcome(&set).unwrap().unwrap_err().code, "invalid");
    let seen = host.calls_of("settings.set");
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].op.as_str(), seen[0].key.as_str()),
        ("settings.set", "run-1:2:0")
    );
    assert_eq!(seen[0].args, vec![json!({"theme": "dark"})]);
    assert_eq!(seen[0].call, clax_surface::wire::CallKind::Write);
    assert_eq!(seen[0].expires_in_ms, Some(30_000));
    assert_eq!(
        (host.applied("run-1:1:0"), host.applied("run-1:2:0")),
        (1, 1)
    );
}

#[tokio::test]
async fn a_result_of_the_wrong_type_is_invalid_result() {
    let (hub, mut host) = desk(|h| h.on("lamps", Behaviour::result(json!({"lamps": 2})))).await;
    let id = call(&hub, &host, "lamps", json!([]), "k");
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(
        hub.outcome(&id).unwrap().unwrap_err().code,
        "invalid_result"
    );
}

#[tokio::test]
async fn a_host_that_has_not_registered_sends_nothing() {
    let hub = MemoryHub::new(HostClock::at(0));
    let mut host = FakeHost::new(hub, "desk", Notation::Dts, &file("desk.d.ts"));
    assert!(matches!(host.serve_once().await, Served::Refused(0, _)));
    assert_eq!(host.rounds(), 0);
}

#[tokio::test]
async fn accepted_then_the_result_after_some_rounds_or_some_time() {
    let (hub, mut host) = desk(|h| {
        h.on(
            "notify",
            Behaviour::Accept {
                when: When::Rounds(2),
                then: Outcome::Result(Value::Null),
            },
        )
        .on(
            "ask",
            Behaviour::Accept {
                when: When::AfterMs(20_000),
                then: Outcome::Result(json!("yes")),
            },
        )
    })
    .await;
    let note = call(&hub, &host, "notify", json!(["Hi."]), "n");
    let ask = call(&hub, &host, "ask", json!(["Tidy?", ["yes", "no"]]), "a");
    assert_eq!(delivered(&host.serve_once().await), 2);
    // Round 2 says `accepted` for both; rounds 2 and 3 let `notify` wait.
    assert_eq!(delivered(&host.serve_once().await), 0);
    assert!(hub.is_open(&note) && hub.is_open(&ask));
    host.serve_once().await;
    assert!(hub.is_open(&note), "two rounds go by");
    host.serve_once().await;
    assert_eq!(hub.outcome(&note), Some(Ok(Value::Null)));
    host.serve_once().await;
    assert!(hub.is_open(&ask), "the clock has not moved");
    hub.clock().advance(20_000);
    host.serve_once().await;
    assert_eq!(hub.outcome(&ask), Some(Ok(json!("yes"))));
    assert_eq!(
        host.calls_of("notify").len(),
        1,
        "an accepted call is not sent again"
    );
}

#[tokio::test]
async fn retry_after_has_the_call_sent_again_later_and_answers_it_then() {
    let then = Box::new(Behaviour::result(Value::Null));
    let (hub, mut host) = desk(|h| h.on("notify", Behaviour::RetryAfter { ms: 5_000, then })).await;
    let id = call(&hub, &host, "notify", json!(["Busy."]), "k");
    assert_eq!(delivered(&host.serve_once().await), 1);
    assert_eq!(delivered(&host.serve_once().await), 0, "asked for later");
    hub.clock().advance(4_999);
    assert_eq!(delivered(&host.serve_once().await), 0, "not yet");
    hub.clock().advance(1);
    assert_eq!(delivered(&host.serve_once().await), 1, "sent again");
    host.serve_once().await;
    assert_eq!(hub.outcome(&id), Some(Ok(Value::Null)));
    assert_eq!(host.applied("k"), 1);
}

#[tokio::test]
async fn a_call_never_answered_expires_as_its_kind_says() {
    let (hub, mut host) = desk(|h| h).await;
    let read = call(&hub, &host, "lamps", json!([]), "r");
    let write = call(&hub, &host, "settings.set", json!([{}]), "w");
    host.serve_once().await;
    hub.clock().advance(30_000);
    assert_eq!(hub.outcome(&read).unwrap().unwrap_err().code, "expired");
    assert_eq!(
        hub.outcome(&write).unwrap().unwrap_err().code,
        "unknown_outcome"
    );
    assert_eq!(host.applied("r") + host.applied("w"), 0);
    assert_eq!(host.serve_until(|h| h.applied("r") > 0, 3).await, Err(3));
}

#[tokio::test]
async fn a_late_answer_lets_the_call_come_again_and_applies_its_key_once() {
    let late = Behaviour::Late {
        when: When::Rounds(3),
        then: Outcome::Result(json!([])),
    };
    let (hub, mut host) = desk(|h| h.on("lamps", late)).await;
    let id = call(&hub, &host, "lamps", json!([]), "k");
    let rounds = host.serve_until(|h| h.applied("k") > 0, 10).await;
    assert_eq!(
        rounds,
        Ok(5),
        "delivered, three rounds unanswered, answered"
    );
    assert_eq!(
        host.calls_of("lamps").len(),
        4,
        "sent with every round until answered"
    );
    assert_eq!(host.times_seen(&id), 4);
    assert!(
        host.calls_of("lamps")
            .iter()
            .all(|c| c.id == id && c.key == "k")
    );
    assert_eq!(hub.outcome(&id), Some(Ok(json!([]))));
    assert_eq!(host.applied("k"), 1);
}

#[tokio::test]
async fn a_dropped_connection_skips_rounds_and_its_host_resumes_its_registration() {
    let accept = Behaviour::Accept {
        when: When::Rounds(0),
        then: Outcome::Result(json!("yes")),
    };
    let (hub, mut host) = desk(|h| h).await;
    let ask = hub
        .call(
            host.registration().unwrap(),
            "run-1",
            "desk.ask",
            vec![json!("Tidy?"), Value::Null],
            "a",
            Some(86_400_000),
        )
        .unwrap();
    host.drop_rounds(2);
    assert_eq!(host.serve_once().await, Served::Skipped);
    assert_eq!(host.serve_once().await, Served::Skipped);
    assert_eq!(host.rounds(), 0);
    assert_eq!(delivered(&host.serve_once().await), 1);
    // Away past the lease: the daemon has dropped the registration.
    hub.clock().advance(400_000);
    match host.serve_once().await {
        Served::Refused(404, body) => assert_eq!(body["error"]["code"], CODE_UNKNOWN_REGISTRATION),
        other => panic!("{other:?}"),
    }
    // A host restarted goes on as the registration it was.
    let before = host.registration().unwrap().to_string();
    let mut host = FakeHost::new(hub.clone(), "desk", Notation::Dts, &file("desk.d.ts"))
        .with_clock(hub.clock().clone())
        .resume(&before)
        .on("ask", accept);
    assert_eq!(host.register().await.unwrap().registration, before);
    // Delivered, then `accepted` and the result in one serve.
    assert_eq!(host.serve_until(|h| h.applied("a") > 0, 5).await, Ok(2));
    assert_eq!(hub.outcome(&ask), Some(Ok(json!("yes"))));
}

#[tokio::test]
async fn a_host_that_resumes_an_id_the_daemon_never_issued_registers_afresh() {
    let hub = MemoryHub::new(HostClock::at(0));
    let never = "0".repeat(32);
    let mut host =
        FakeHost::new(hub.clone(), "desk", Notation::Dts, &file("desk.d.ts")).resume(&never);
    let registered = host.register().await.unwrap();
    assert_ne!(registered.registration, never);
    assert_eq!(host.registration(), Some(registered.registration.as_str()));
}

#[tokio::test]
async fn an_error_code_the_operation_does_not_declare_is_invalid_result() {
    let (hub, mut host) = desk(|h| {
        h.on("lamps", Behaviour::throws_undeclared())
            .on("settings.set", Behaviour::error("unavailable", "busy"))
    })
    .await;
    let lamps = call(&hub, &host, "lamps", json!([]), "l");
    let set = call(&hub, &host, "settings.set", json!([{}]), "s");
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(
        hub.outcome(&lamps).unwrap().unwrap_err().code,
        "invalid_result"
    );
    let unavailable = hub.outcome(&set).unwrap().unwrap_err();
    assert_eq!(
        (unavailable.code.as_str(), unavailable.retryable),
        ("unavailable", true)
    );
}

#[tokio::test]
async fn an_end_the_host_does_not_know_is_recorded_and_acknowledged() {
    let (hub, mut host) = desk(|h| h).await;
    let mut other = serde_json::Map::new();
    other.insert("evaporated".to_string(), json!({"how": "slowly"}));
    let end = hub.end(
        host.registration().unwrap(),
        "run-9",
        End::Other(other.clone()),
        None,
    );
    assert_eq!(delivered(&host.serve_once().await), 1);
    assert_eq!(delivered(&host.serve_once().await), 0, "acknowledged");
    let ended = host.ended();
    assert_eq!(
        (ended[0].id.as_str(), &ended[0].end),
        (end.as_str(), &End::Other(other))
    );
    assert_eq!(hub.held(), (0, 0));
}

#[tokio::test]
async fn a_second_answer_and_one_for_an_unknown_id_change_nothing() {
    let (hub, mut host) =
        desk(|h| h.on("lamps", Behaviour::Twice(Outcome::Result(json!([]))))).await;
    let id = call(&hub, &host, "lamps", json!([]), "k");
    host.serve_once().await;
    host.answer("no-such-call", Outcome::Result(json!(1)));
    assert_eq!(delivered(&host.serve_once().await), 0);
    assert_eq!(hub.outcome(&id), Some(Ok(json!([]))));
    assert_eq!(
        delivered(&host.serve_once().await),
        0,
        "the second answer is taken and ignored"
    );
    assert_eq!(host.applied("k"), 1);
    assert_eq!(hub.held(), (0, 0));
}

#[tokio::test]
async fn a_call_made_again_under_an_applied_key_is_answered_the_same_unapplied() {
    let (hub, mut host) = desk(|h| h.on("settings.set", Behaviour::result(Value::Null))).await;
    let first = call(&hub, &host, "settings.set", json!([{"theme": "dark"}]), "k");
    assert_eq!(
        call(&hub, &host, "settings.set", json!([{"theme": "dark"}]), "k"),
        first
    );
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(hub.outcome(&first), Some(Ok(Value::Null)));
    // The run read its answer, so the hub forgot the key: the same call
    // made again (a step run again after a crash) is a new delivery.
    let again = call(&hub, &host, "settings.set", json!([{"theme": "dark"}]), "k");
    assert_ne!(again, first);
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(hub.outcome(&again), Some(Ok(Value::Null)));
    assert_eq!(host.calls_of("settings.set").len(), 2);
    assert_eq!(host.applied("k"), 1, "applied once, however often it came");
}

#[tokio::test]
async fn an_ended_run_is_recorded_and_acknowledged() {
    let (hub, mut host) = desk(|h| h).await;
    let run = host
        .start_run(
            "tidy",
            json!({"ask": "Tidy up."}),
            Some(vec!["desk.*".into()]),
        )
        .await
        .unwrap();
    assert_eq!(run, "run-1");
    let started = hub.started();
    assert_eq!(Some(started[0].registration.as_str()), host.registration());
    assert_eq!(
        (started[0].kind.as_str(), started[0].allow.as_deref()),
        ("tidy", Some(&["desk.*".to_string()][..]))
    );
    let end = hub.end(
        host.registration().unwrap(),
        &run,
        End::Finished {},
        Some(json!({"summary": "Tidied."})),
    );
    assert_eq!(delivered(&host.serve_once().await), 1);
    assert_eq!(delivered(&host.serve_once().await), 0, "acknowledged");
    let ended = host.ended();
    assert_eq!((ended.len(), ended[0].id.as_str()), (1, end.as_str()));
    assert_eq!(ended[0].result, Some(json!({"summary": "Tidied."})));
    assert_eq!(hub.held(), (0, 0));
}

#[test]
fn a_behaviour_reads_from_json() {
    assert_eq!(Behaviour::from_json(&json!("never")), Ok(Behaviour::Never));
    assert_eq!(
        Behaviour::from_json(&json!({"result": [1]})),
        Ok(Behaviour::result(json!([1])))
    );
    assert_eq!(
        Behaviour::from_json(&json!({"error": {"code": "declined", "message": "No."}})),
        Ok(Behaviour::error("declined", "No."))
    );
    assert_eq!(
        Behaviour::from_json(&json!({"accept": {"result": null}, "rounds": 2})),
        Ok(Behaviour::Accept {
            when: When::Rounds(2),
            then: Outcome::Result(Value::Null)
        })
    );
    assert!(Behaviour::from_json(&json!({"result": 1, "accepted": true})).is_err());
}
