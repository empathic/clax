//! A call sent again, by the hub in process ([`MemoryHub`]) and a
//! [`FakeHost`]: one test for each row of the two tables of the design's
//! "Deadlines, retries and what a call may promise"
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`), and
//! the bounds of sending again. Every wait is the hub's clock, which only
//! the test moves.

use super::*;
use clax_bot::bot_surfaces::{CallFailure, HubLimits, Retry};
use clax_surface::wire::Call;

const FIXTURE: &str = include_str!("../../../../docs/claxbot-surfaces/contract.json");

/// Three attempts, one second and then two apart.
const RULE: Retry = Retry {
    attempts: 3,
    backoff_ms: 1_000,
    backoff_max_ms: 4_000,
};

fn desk_dts() -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("the fixture is JSON");
    fixture["files"]["desk.d.ts"]
        .as_str()
        .expect("a file")
        .to_string()
}

/// A hub at 0 whose calls are sent again as `RULE` says, with `limits`,
/// and a host of `desk` on it, registered.
async fn desk_with(
    limits: HubLimits,
    host: impl FnOnce(FakeHost<MemoryHub>) -> FakeHost<MemoryHub>,
) -> (MemoryHub, FakeHost<MemoryHub>) {
    let hub = MemoryHub::with_limits(HostClock::at(0), limits).retrying(RULE);
    let mut host = host(
        FakeHost::new(hub.clone(), "desk", Notation::Dts, &desk_dts())
            .with_clock(hub.clock().clone()),
    );
    host.register().await.expect("desk registers");
    (hub, host)
}

async fn desk(
    host: impl FnOnce(FakeHost<MemoryHub>) -> FakeHost<MemoryHub>,
) -> (MemoryHub, FakeHost<MemoryHub>) {
    desk_with(HubLimits::default(), host).await
}

/// `run`'s call of `desk.<op>` with `ms` to answer.
fn call_of(
    hub: &MemoryHub,
    registration: &str,
    run: &str,
    op: &str,
    args: Value,
    key: &str,
    ms: u64,
) -> String {
    hub.call(
        registration,
        run,
        &format!("desk.{op}"),
        args.as_array().unwrap().clone(),
        key,
        Some(hub.clock().now() + ms),
    )
    .expect("the call is opened")
}

fn call(hub: &MemoryHub, host: &FakeHost<MemoryHub>, op: &str, args: Value, key: &str) -> String {
    call_of(
        hub,
        host.registration().unwrap(),
        "run-1",
        op,
        args,
        key,
        30_000,
    )
}

fn delivered(served: &Served) -> usize {
    match served {
        Served::Delivered(d) => d.iter().filter(|d| matches!(d, Delivery::Call(_))).count(),
        other => panic!("not delivered: {other:?}"),
    }
}

fn failure(hub: &MemoryHub, id: &str) -> CallFailure {
    match hub.outcome(id) {
        Some(Err(failure)) => failure,
        other => panic!("not a failure: {other:?}"),
    }
}

/// Every time `id` came, with its key.
fn sent<'a>(host: &'a FakeHost<MemoryHub>, op: &str, id: &str) -> Vec<&'a Call> {
    host.calls_of(op)
        .into_iter()
        .filter(|c| c.id == id)
        .collect()
}

// ---------------------------------------------------------------------------
// What repeating an operation means, when no answer comes
// ---------------------------------------------------------------------------

/// `@read`: harmless to send again. A host that accepted a read and
/// started again is sent it again, with its ID and key.
#[tokio::test]
async fn a_read_its_host_lost_is_sent_again() {
    let accept = || Behaviour::Accept {
        when: When::Rounds(1),
        then: Outcome::Result(json!([])),
    };
    let (hub, mut host) = desk(|h| h.on("lamps", Behaviour::Never)).await;
    let id = call(&hub, &host, "lamps", json!([]), "r");
    assert_eq!(delivered(&host.serve_once().await), 1);
    host.answer(&id, Outcome::Accepted);
    host.serve_once().await;
    assert!(hub.is_open(&id));
    // It starts again, and now answers.
    host.restart();
    let mut host = host.on("lamps", accept());
    host.register().await.unwrap();
    assert_eq!(
        delivered(&host.serve_once().await),
        0,
        "not before its wait"
    );
    hub.clock().advance(1_000);
    assert_eq!(delivered(&host.serve_once().await), 1, "sent again");
    host.serve_until(|_| !hub.is_open(&id), 4).await.unwrap();
    assert_eq!(hub.outcome(&id), Some(Ok(json!([]))));
    let keys: Vec<&str> = sent(&host, "lamps", &id)
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, ["r", "r"]);
}

/// `@write` and `@effect`: applied once under a key, so sent again with
/// it. A host that started again after accepting a write is sent it
/// again, and applies its key once.
#[tokio::test]
async fn a_write_or_an_effect_its_host_lost_is_sent_again_with_its_key() {
    let later = || Behaviour::Accept {
        when: When::Rounds(1),
        then: Outcome::Result(Value::Null),
    };
    let (hub, mut host) = desk(|h| {
        h.on(
            "settings.set",
            Behaviour::Accept {
                when: When::Rounds(100),
                then: Outcome::Result(Value::Null),
            },
        )
        .on(
            "notify",
            Behaviour::Accept {
                when: When::Rounds(100),
                then: Outcome::Result(Value::Null),
            },
        )
    })
    .await;
    let write = call(&hub, &host, "settings.set", json!([{"theme": "dark"}]), "w");
    let effect = call(&hub, &host, "notify", json!(["Done."]), "e");
    host.serve_once().await;
    host.serve_once().await;
    assert!(hub.is_open(&write) && hub.is_open(&effect));
    host.restart();
    let mut host = host.on("settings.set", later()).on("notify", later());
    host.register().await.unwrap();
    assert_eq!(
        delivered(&host.serve_once().await),
        0,
        "not before its wait"
    );
    hub.clock().advance(1_000);
    host.serve_until(|_| !hub.is_open(&write) && !hub.is_open(&effect), 6)
        .await
        .unwrap();
    assert_eq!(hub.outcome(&write), Some(Ok(Value::Null)));
    assert_eq!(hub.outcome(&effect), Some(Ok(Value::Null)));
    for (op, id, key) in [("settings.set", &write, "w"), ("notify", &effect, "e")] {
        let calls = sent(&host, op, id);
        assert_eq!(calls.len(), 2, "{op}: sent again once");
        assert!(calls.iter().all(|c| c.key == key && &c.id == id));
        assert_eq!(host.applied(key), 1, "{op}: applied once");
    }
}

/// `@effect @once`: the host cannot tell a repeat, so a call it accepted
/// is never sent again, also after it started again: at its deadline
/// the script is thrown `unknown_outcome`.
#[tokio::test]
async fn an_effect_tagged_once_is_never_sent_again_once_accepted() {
    let (hub, mut host) = desk(|h| {
        h.on(
            "ask",
            Behaviour::Accept {
                when: When::Rounds(100),
                then: Outcome::Result(json!("yes")),
            },
        )
    })
    .await;
    let ask = call(&hub, &host, "ask", json!(["Tidy?"]), "a");
    host.serve_once().await;
    host.serve_once().await;
    host.restart();
    host.register().await.unwrap();
    for _ in 0..5 {
        hub.clock().advance(4_000);
        host.serve_once().await;
    }
    assert_eq!(sent(&host, "ask", &ask).len(), 1, "never sent again");
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &ask);
    assert_eq!(
        (thrown.code.as_str(), thrown.retryable),
        ("unknown_outcome", false)
    );
}

/// `@effect @once`, accepted, and then failed by its host with an error
/// that says it may be tried again: the call is accepted, so it is never
/// sent again, and the error is its outcome at once, whether the two
/// answers come in one `serve` or in two.
#[tokio::test]
async fn an_accepted_once_call_failed_retryably_is_not_sent_again_in_one_serve_or_two() {
    for together in [true, false] {
        let (hub, mut host) = desk(|h| h.on("ask", Behaviour::Never)).await;
        let ask = call(&hub, &host, "ask", json!(["Tidy?"]), "a");
        assert_eq!(delivered(&host.serve_once().await), 1);
        host.answer(&ask, Outcome::Accepted);
        if !together {
            host.serve_once().await;
            assert!(hub.is_open(&ask), "accepted");
        }
        host.answer(
            &ask,
            Outcome::Error(CallError::new("declined", "busy").retryable()),
        );
        host.serve_once().await;
        for _ in 0..3 {
            hub.clock().advance(2_000);
            host.serve_once().await;
        }
        assert_eq!(
            sent(&host, "ask", &ask).len(),
            1,
            "together: {together}: never sent again"
        );
        let thrown = failure(&hub, &ask);
        assert_eq!(
            (thrown.code.as_str(), thrown.retryable),
            ("declined", true),
            "together: {together}"
        );
    }
}

/// Two retryable errors for one attempt, in one `serve`: the second
/// finds the next attempt planned and takes no attempt more, so the call
/// is sent again after the first wait, not the second.
#[tokio::test]
async fn two_errors_for_one_attempt_spend_one_attempt() {
    let (hub, mut host) = desk(|h| h.on("settings.set", Behaviour::Never)).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    assert_eq!(delivered(&host.serve_once().await), 1);
    for _ in 0..2 {
        host.answer(
            &id,
            Outcome::Error(CallError::new("invalid", "busy").retryable()),
        );
    }
    assert_eq!(delivered(&host.serve_once().await), 0, "its wait first");
    hub.clock().advance(1_000);
    assert_eq!(
        delivered(&host.serve_once().await),
        1,
        "after the first wait"
    );
}

// ---------------------------------------------------------------------------
// What the script sees when the policy is spent
// ---------------------------------------------------------------------------

/// `unavailable`: no host took the call. A host that is not connected
/// is waited for until the call's deadline, and sent nothing meanwhile.
#[tokio::test]
async fn a_call_no_host_took_is_unavailable_and_nothing_happened() {
    let (hub, mut host) = desk(|h| h.on("notify", Behaviour::result(Value::Null))).await;
    host.serve_once().await;
    hub.clock().advance(HubLimits::default().connected_ms + 1);
    let id = call(&hub, &host, "notify", json!(["Hi."]), "n");
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &id);
    assert_eq!(
        (thrown.code.as_str(), thrown.retryable),
        ("unavailable", true)
    );
    assert!(
        thrown.message.contains("Nothing happened"),
        "{}",
        thrown.message
    );
    assert_eq!(host.applied("n"), 0);
    // Under a rule that sends nothing again, a host that is away takes
    // no call at all.
    let once = hub.clone().retrying(Retry::NONE);
    let refused = once.call(
        host.registration().unwrap(),
        "run-1",
        "desk.notify",
        vec![json!("Hi.")],
        "n2",
        Some(hub.clock().now() + 30_000),
    );
    assert_eq!(refused.unwrap_err().code, "unavailable");
}

/// `expired`: a read was accepted and not answered in time.
#[tokio::test]
async fn a_read_accepted_and_not_answered_expires() {
    let accept = Behaviour::Accept {
        when: When::Rounds(1_000),
        then: Outcome::Result(json!([])),
    };
    let (hub, mut host) = desk(|h| h.on("lamps", accept)).await;
    let id = call(&hub, &host, "lamps", json!([]), "r");
    host.serve_once().await;
    host.serve_once().await;
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &id);
    assert_eq!((thrown.code.as_str(), thrown.retryable), ("expired", false));
}

/// `unknown_outcome`: a write or an effect was accepted and not
/// answered. It may have taken effect.
#[tokio::test]
async fn a_write_accepted_and_not_answered_has_an_unknown_outcome() {
    let accept = Behaviour::Accept {
        when: When::Rounds(1_000),
        then: Outcome::Result(Value::Null),
    };
    let (hub, mut host) = desk(|h| h.on("settings.set", accept)).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    host.serve_once().await;
    host.serve_once().await;
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &id);
    assert_eq!(
        (thrown.code.as_str(), thrown.retryable),
        ("unknown_outcome", false)
    );
}

// ---------------------------------------------------------------------------
// A host's account, and the bounds of sending again
// ---------------------------------------------------------------------------

/// A host's error that says it may be tried again: the call is sent
/// again after the rule's wait, under its ID and key, and the result
/// that follows is the script's. Nothing is sent again before the wait.
#[tokio::test]
async fn a_retryable_error_is_sent_again_after_its_wait_and_then_succeeds() {
    let flaky = Behaviour::fails(1, "invalid", Behaviour::result(Value::Null));
    let (hub, mut host) = desk(|h| h.on("settings.set", flaky)).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    assert_eq!(delivered(&host.serve_once().await), 1);
    assert_eq!(delivered(&host.serve_once().await), 0, "its wait first");
    assert!(hub.is_open(&id));
    hub.clock().advance(999);
    assert_eq!(delivered(&host.serve_once().await), 0, "not yet");
    hub.clock().advance(1);
    assert_eq!(delivered(&host.serve_once().await), 1, "sent again");
    host.serve_once().await;
    assert_eq!(hub.outcome(&id), Some(Ok(Value::Null)));
    assert_eq!(host.failures(), 1);
    assert_eq!(host.applied("w"), 1);
    assert!(
        sent(&host, "settings.set", &id)
            .iter()
            .all(|c| c.key == "w")
    );
}

/// The attempts spent: the script is thrown the host's last error, which
/// says retrying may help. The waits between attempts grow.
#[tokio::test]
async fn attempts_spent_throw_the_hosts_error() {
    let always = Behaviour::fails(100, "invalid", Behaviour::Never);
    let (hub, mut host) = desk(|h| h.on("settings.set", always)).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    let mut at = Vec::new();
    for _ in 0..20 {
        if delivered(&host.serve_once().await) > 0 {
            at.push(hub.clock().now());
        }
        hub.clock().advance(500);
    }
    // Each error comes with the round after its delivery, half a second
    // later; the next attempt is a second after the first error, and
    // two after the second.
    assert_eq!(at, [0, 1_500, 4_000], "three attempts");
    let thrown = failure(&hub, &id);
    assert_eq!((thrown.code.as_str(), thrown.retryable), ("invalid", true));
    assert_eq!(host.failures(), 3);
}

/// A wait that would end at or after the deadline is not waited: the
/// script is thrown the host's error at once. One that ends before it,
/// whose host is then away, is thrown that error at the deadline: the
/// call was not sent again, and the error is what is known of it.
#[tokio::test]
async fn the_deadline_reached_in_a_wait_throws_the_last_error() {
    let fails = Behaviour::fails(100, "invalid", Behaviour::Never);
    let (hub, mut host) = desk(|h| h.on("settings.set", fails)).await;
    let short = call_of(
        &hub,
        host.registration().unwrap(),
        "run-1",
        "settings.set",
        json!([{}]),
        "w1",
        800,
    );
    host.serve_once().await;
    host.serve_once().await;
    let thrown = failure(&hub, &short);
    assert_eq!((thrown.code.as_str(), thrown.retryable), ("invalid", true));
    assert_eq!(hub.clock().now(), 0, "settled at once, before its deadline");

    let long = call(&hub, &host, "settings.set", json!([{}]), "w2");
    host.serve_once().await;
    host.serve_once().await;
    assert!(hub.is_open(&long), "waits a second");
    host.drop_rounds(100);
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &long);
    assert_eq!((thrown.code.as_str(), thrown.retryable), ("invalid", true));
    assert_eq!(sent(&host, "settings.set", &long).len(), 1);
}

/// An error that does not say it may be tried again is final: thrown at
/// once, not retryable, and the call is sent once.
#[tokio::test]
async fn a_final_error_is_not_sent_again() {
    let (hub, mut host) = desk(|h| h.on("settings.set", Behaviour::error("invalid", "No."))).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    host.serve_once().await;
    host.serve_once().await;
    let thrown = failure(&hub, &id);
    assert_eq!((thrown.code.as_str(), thrown.retryable), ("invalid", false));
    for _ in 0..4 {
        hub.clock().advance(4_000);
        host.serve_once().await;
    }
    assert_eq!(sent(&host, "settings.set", &id).len(), 1);
}

/// `@once`, accepted, and then its host is gone for good: never sent
/// again, and the script is thrown `unknown_outcome` at the deadline
/// whatever the host's other calls do.
#[tokio::test]
async fn a_once_call_accepted_then_its_host_gone_has_an_unknown_outcome() {
    let (hub, mut host) = desk(|h| {
        h.on(
            "ask",
            Behaviour::Accept {
                when: When::Rounds(1_000),
                then: Outcome::Result(json!("yes")),
            },
        )
    })
    .await;
    let ask = call(&hub, &host, "ask", json!(["Tidy?"]), "a");
    host.serve_once().await;
    host.serve_once().await;
    host.drop_rounds(1_000);
    hub.clock().advance(30_000);
    let thrown = failure(&hub, &ask);
    assert_eq!(thrown.code, "unknown_outcome");
    assert_eq!(sent(&host, "ask", &ask).len(), 1);
}

/// The hub starts again (a daemon's restart) while a call waits out its
/// wait after a retryable error: the call is in the store, and is sent
/// to its host when it goes on as its registration, under its ID and
/// key.
#[tokio::test]
async fn a_hub_that_starts_again_sends_a_call_in_its_wait_when_its_host_is_back() {
    let flaky = Behaviour::fails(1, "invalid", Behaviour::result(Value::Null));
    let (hub, mut host) = desk(|h| h.on("settings.set", flaky)).await;
    let id = call(&hub, &host, "settings.set", json!([{}]), "w");
    host.serve_once().await;
    host.serve_once().await;
    assert!(hub.is_open(&id));
    hub.restart(HubLimits::default());
    assert!(matches!(host.serve_once().await, Served::Refused(404, _)));
    host.register().await.expect("goes on as the registration");
    hub.clock().advance(1_000);
    host.serve_until(|_| !hub.is_open(&id), 4).await.unwrap();
    assert_eq!(hub.outcome(&id), Some(Ok(Value::Null)));
    let calls = sent(&host, "settings.set", &id);
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|c| c.key == "w"));
    assert_eq!(host.applied("w"), 1);
}

/// A host that comes and goes: each time it comes back it may have lost
/// the write it accepted, which is sent again as the rule allows and no
/// more: its attempts are the call's, its waits grow, and nothing is
/// sent while it is away.
#[tokio::test]
async fn a_host_that_comes_and_goes_is_sent_its_call_as_often_as_the_rule_allows() {
    let accept = Behaviour::Accept {
        when: When::Rounds(1_000),
        then: Outcome::Result(Value::Null),
    };
    let (hub, mut host) = desk(|h| h.on("settings.set", accept)).await;
    let id = call_of(
        &hub,
        host.registration().unwrap(),
        "run-1",
        "settings.set",
        json!([{}]),
        "w",
        600_000,
    );
    host.serve_once().await;
    host.serve_once().await;
    let away = HubLimits::default().connected_ms + 1;
    let mut seen = vec![sent(&host, "settings.set", &id).len()];
    for _ in 0..6 {
        // Away: no serve, so nothing is sent.
        hub.clock().advance(away);
        // Back, and a few serves a second apart.
        for _ in 0..3 {
            host.serve_once().await;
            hub.clock().advance(1_000);
        }
        seen.push(sent(&host, "settings.set", &id).len());
    }
    assert_eq!(seen, [1, 2, 3, 3, 3, 3, 3], "two more attempts, then none");
    assert!(hub.is_open(&id), "accepted: its deadline stands");
}

/// A run's calls are sent again no more than the hub's bound for a run,
/// across its calls: past it, a host's retryable error is the call's
/// outcome at once.
#[tokio::test]
async fn a_runs_calls_are_sent_again_no_more_than_its_bound() {
    let limits = HubLimits {
        resends_per_run: 2,
        ..HubLimits::default()
    };
    let fails = || Behaviour::fails(100, "invalid", Behaviour::Never);
    let (hub, mut host) = desk_with(limits, |h| h.on("settings.set", fails())).await;
    let a = call(&hub, &host, "settings.set", json!([{}]), "a");
    let b = call(&hub, &host, "settings.set", json!([{}]), "b");
    host.serve_once().await;
    host.serve_once().await;
    assert!(hub.is_open(&a) && hub.is_open(&b), "one resend each");
    hub.clock().advance(1_000);
    host.serve_once().await;
    host.serve_once().await;
    assert_eq!(failure(&hub, &a).code, "invalid");
    assert_eq!(failure(&hub, &b).code, "invalid");
    // Another run has its own.
    let c = call_of(
        &hub,
        host.registration().unwrap(),
        "run-2",
        "settings.set",
        json!([{}]),
        "c",
        30_000,
    );
    host.serve_once().await;
    host.serve_once().await;
    assert!(hub.is_open(&c));
}

/// `retry_after_ms` is the host's account and no failure: it spends no
/// attempt and none of the run's sendings again. Seventy answers of it
/// leave the call open, sent again each time after the rule's first
/// wait (`retry_after_ms: 0` is not sent again at once), and a run whose
/// bound of sendings again is one still has it for the host's retryable
/// error after them.
#[tokio::test]
async fn retry_after_spends_nothing_and_waits_at_least_the_rules_first_wait() {
    let limits = HubLimits {
        resends_per_run: 1,
        ..HubLimits::default()
    };
    let (hub, mut host) = desk_with(limits, |h| h.on("settings.set", Behaviour::Never)).await;
    let id = call_of(
        &hub,
        host.registration().unwrap(),
        "run-1",
        "settings.set",
        json!([{}]),
        "w",
        600_000,
    );
    assert_eq!(delivered(&host.serve_once().await), 1);
    for n in 0..70 {
        host.answer(&id, Outcome::RetryAfter(0));
        assert_eq!(delivered(&host.serve_once().await), 0, "{n}: not at once");
        hub.clock().advance(999);
        assert_eq!(delivered(&host.serve_once().await), 0, "{n}: not yet");
        hub.clock().advance(1);
        assert_eq!(delivered(&host.serve_once().await), 1, "{n}: sent again");
    }
    assert!(hub.is_open(&id), "open until its deadline");
    host.answer(
        &id,
        Outcome::Error(CallError::new("invalid", "busy").retryable()),
    );
    host.serve_once().await;
    hub.clock().advance(1_000);
    assert_eq!(
        delivered(&host.serve_once().await),
        1,
        "the run's sending again is left"
    );
}

/// A hub whose runs give no rule ([`Retry::NONE`]), and a host of `desk`
/// on it, registered: a host that never answers `retryable`.
async fn old_desk(
    host: impl FnOnce(FakeHost<MemoryHub>) -> FakeHost<MemoryHub>,
) -> (MemoryHub, FakeHost<MemoryHub>) {
    let hub = MemoryHub::new(HostClock::at(0));
    let mut host = host(
        FakeHost::new(hub.clone(), "desk", Notation::Dts, &desk_dts())
            .with_clock(hub.clock().clone()),
    );
    host.register().await.expect("desk registers");
    (hub, host)
}

/// Under [`Retry::NONE`] a host is served as before rules were given: a
/// `@waits` call it answers `retry_after_ms: 60000` for is sent again
/// each minute, seventy times, and stays open until its 24-hour
/// deadline; `retry_after_ms: 0` is sent again at once; `unavailable`
/// settles a call at once; and an accepted call is not sent again when
/// its host comes back, after it was away or after the hub started
/// again.
#[tokio::test]
async fn an_old_host_is_served_as_before_rules_were_given() {
    let day = 24 * 60 * 60 * 1000;
    let (hub, mut host) = old_desk(|h| {
        h.on("ask", Behaviour::Never)
            .on("lamps", Behaviour::error("unavailable", "busy"))
            .on(
                "settings.set",
                Behaviour::Accept {
                    when: When::Rounds(100_000),
                    then: Outcome::Result(Value::Null),
                },
            )
    })
    .await;
    let registration = host.registration().unwrap().to_string();
    let ask = call_of(
        &hub,
        &registration,
        "run-1",
        "ask",
        json!(["Tidy?"]),
        "a",
        day,
    );
    assert_eq!(delivered(&host.serve_once().await), 1);
    for n in 0..70 {
        host.answer(&ask, Outcome::RetryAfter(60_000));
        assert_eq!(
            delivered(&host.serve_once().await),
            0,
            "{n}: asked for later"
        );
        hub.clock().advance(60_000);
        assert_eq!(
            delivered(&host.serve_once().await),
            1,
            "{n}: sent when asked"
        );
    }
    assert!(hub.is_open(&ask), "open after seventy minutes");
    host.answer(&ask, Outcome::RetryAfter(0));
    assert_eq!(delivered(&host.serve_once().await), 1, "sent again at once");

    let lamps = call(&hub, &host, "lamps", json!([]), "l");
    host.serve_once().await;
    host.serve_once().await;
    let thrown = failure(&hub, &lamps);
    assert_eq!(
        (thrown.code.as_str(), thrown.retryable),
        ("unavailable", true),
        "settled at once"
    );
    assert_eq!(sent(&host, "lamps", &lamps).len(), 1);

    let write = call_of(
        &hub,
        &registration,
        "run-1",
        "settings.set",
        json!([{}]),
        "w",
        day,
    );
    host.serve_once().await;
    host.serve_once().await;
    // Away, and back.
    hub.clock().advance(HubLimits::default().connected_ms + 1);
    for _ in 0..3 {
        host.serve_once().await;
        hub.clock().advance(4_000);
    }
    // The hub starts again, and the host goes on as its registration.
    hub.restart(HubLimits::default());
    host.restart();
    host.register().await.expect("goes on as the registration");
    for _ in 0..3 {
        host.serve_once().await;
        hub.clock().advance(4_000);
    }
    assert_eq!(
        sent(&host, "settings.set", &write).len(),
        1,
        "never sent again"
    );
    assert!(hub.is_open(&write));
    hub.clock().set(day);
    assert_eq!(
        failure(&hub, &ask).code,
        "unknown_outcome",
        "at its deadline"
    );
}

/// The hub starts again: a call it finds in the store is under no rule
/// it was given ([`HubLimits::retry_kept`], [`Retry::NONE`]), so a write
/// its host had accepted is not sent again when the host goes on as its
/// registration. Once its run makes it again with its key, the run's
/// rule takes over: the next time the host may have lost it (it was
/// away), it is sent again after the rule's wait.
#[tokio::test]
async fn a_call_made_again_after_a_restart_goes_on_under_its_runs_rule() {
    let accept = Behaviour::Accept {
        when: When::Rounds(100_000),
        then: Outcome::Result(Value::Null),
    };
    let (hub, mut host) = desk(|h| h.on("settings.set", accept)).await;
    let registration = host.registration().unwrap().to_string();
    let made = || {
        call_of(
            &hub,
            &registration,
            "run-1",
            "settings.set",
            json!([{}]),
            "w",
            600_000,
        )
    };
    let id = made();
    host.serve_once().await;
    host.serve_once().await;
    hub.restart(HubLimits::default());
    host.restart();
    host.register().await.expect("goes on as the registration");
    for _ in 0..3 {
        host.serve_once().await;
        hub.clock().advance(2_000);
    }
    assert_eq!(sent(&host, "settings.set", &id).len(), 1, "no rule given");
    assert_eq!(made(), id, "made again with its key");
    hub.clock().advance(HubLimits::default().connected_ms + 1);
    assert_eq!(delivered(&host.serve_once().await), 0, "its wait first");
    hub.clock().advance(1_000);
    assert_eq!(delivered(&host.serve_once().await), 1, "the run's rule");
}

/// The bound of sendings again is a run's for each registration it
/// calls: one host's failures spend none of another's in a run both
/// serve.
#[tokio::test]
async fn a_runs_bound_of_sendings_again_is_its_own_for_each_registration() {
    let limits = HubLimits {
        resends_per_run: 1,
        ..HubLimits::default()
    };
    let fails = || Behaviour::fails(100, "invalid", Behaviour::Never);
    let (hub, mut one) = desk_with(limits, |h| h.on("settings.set", fails())).await;
    let mut other = FakeHost::new(hub.clone(), "desk", Notation::Dts, &desk_dts())
        .with_clock(hub.clock().clone())
        .on("settings.set", fails());
    other.register().await.expect("a second registration");
    assert_ne!(one.registration(), other.registration());
    let a = call(&hub, &one, "settings.set", json!([{}]), "a");
    one.serve_once().await;
    one.serve_once().await;
    assert!(hub.is_open(&a), "one's sending again");
    let b = call(&hub, &other, "settings.set", json!([{}]), "b");
    other.serve_once().await;
    other.serve_once().await;
    assert!(hub.is_open(&b), "the other's own sending again");
    // One's is spent.
    let c = call(&hub, &one, "settings.set", json!([{}]), "c");
    one.serve_once().await;
    one.serve_once().await;
    assert_eq!(failure(&hub, &c).code, "invalid");
}
