//! A host's messages to a run it started, and what it is told of the run
//! (`docs/contract.md`, "ClaxBot surfaces"; `clax_server::routes::bot_messages`):
//! over HTTP, with the testkit's host, a scripted model and the daemon's
//! own driver, by a clock and sleeps the test moves.
//!
//! The surface is [`INBOX`], a test's own: its kind `chat` takes
//! messages, `once` takes none.

use clax_bot::clax_surface::wire::{End, Notation, Outcome, RunState};
use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_testkit::fake_host::{Behaviour, FakeHost, HttpTransport, When};
use clax_testkit::fake_messages::{FakeMessages, Turn};
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";
const START: &str = "2026-11-11T00:00:00Z";
/// How long a script waits for a call that may wait before its run parks
/// (`clax_bot::Delivery::DAEMON`).
const PARK_AFTER: Duration = Duration::from_secs(10);
/// The time a driver is given to become idle; no test comes near it.
const BUDGET: Duration = Duration::from_secs(3600);

const INBOX: &str = r#"/** An inbox to work through. */
declare const inbox: {
  /** The items waiting. @read */
  items(): Promise<string[]>;
  /** Asks the person, and waits for their answer. @effect @waits @once @throws declined */
  ask(question: string): Promise<string>;
};
interface Kinds {
  /** The request is what the person wants done with the inbox. @allow inbox.* */
  chat(request: { ask: Instruction }, message?: { say: Instruction; note: string }): { summary: string };
  /** The request is one thing to do. @allow inbox.* */
  once(request: { ask: Instruction }): { summary: string };
}
"#;

const ASK: &str = r#"return await clax.inbox.ask("Which first?");"#;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A daemon with its driver, ticked by the test, a scripted model and a
/// registered host of [`INBOX`].
struct Daemon {
    ts: TestServer,
    clock: Arc<ManualClock>,
    sleeper: Arc<ManualSleeper>,
    driver: Arc<clax_bot::Driver>,
    fake: FakeMessages,
    host: FakeHost<HttpTransport>,
}

async fn daemon(turns: Vec<Turn>) -> Daemon {
    let fake = FakeMessages::start(turns).await;
    let clock = Arc::new(ManualClock::at(START));
    let sleeper = Arc::new(ManualSleeper::default());
    let slot = Arc::new(Mutex::new(None));
    let (s, kept) = (sleeper.clone(), slot.clone());
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces.set_sleeper(s);
        *kept.lock().unwrap() = Some(clax_server::bot::driver(state));
    })
    .await;
    let driver = slot.lock().unwrap().take().expect("a driver");
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(
                clax_store::bot_settings::Setting::BaseUrl,
                Some(json!(fake.base)),
            )],
        )
        .unwrap();
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!(s, 200, "{v}");
    let mut host = inbox_host(&ts)
        .on("items", Behaviour::result(json!(["a", "b"])))
        .on(
            "ask",
            Behaviour::Accept {
                when: When::AfterMs(1),
                then: Outcome::Result(json!("the oldest")),
            },
        )
        .wait_ms(0);
    host.register().await.expect("inbox registers");
    Daemon {
        ts,
        clock,
        sleeper,
        driver,
        fake,
        host,
    }
}

fn inbox_host(ts: &TestServer) -> FakeHost<HttpTransport> {
    FakeHost::new(HttpTransport::of(ts), "inbox", Notation::Dts, INBOX)
}

impl Daemon {
    async fn tick(&self) {
        self.driver.run_until_idle(BUDGET).await;
    }

    /// Ticks the driver while the host serves, until the run parks on
    /// the question: the script waits as long as one may, and the run
    /// parks.
    async fn tick_until_parked(&mut self) {
        let ticking = {
            let driver = self.driver.clone();
            tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
        };
        self.sleeper.wait_for(PARK_AFTER, 1).await;
        self.clock.advance_millis(10_000);
        self.sleeper.fire(PARK_AFTER);
        ticking.await.unwrap();
    }

    async fn state(&self, run: &str) -> RunState {
        self.host.run_state(run).await.expect("the run's state")
    }

    /// The text of every user text block of model request `n`.
    fn sent(&self, n: usize) -> String {
        let body = &self.fake.requests()[n];
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "user")
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .filter_map(|b| b["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n----\n")
    }

    /// The events the host was delivered for `run`.
    fn events(&self, run: &str) -> Vec<Value> {
        self.host
            .events()
            .into_iter()
            .filter(|e| e.run == run)
            .map(|e| e.event.clone())
            .collect()
    }
}

/// A message is given to the run's model once, at the turn after it
/// arrived, with the person's words marked; one sent while the run is
/// parked leaves it parked on its question, which stays open, and is
/// answered `waiting`: it is given at the turn after the question is
/// answered. The host is told each step, the park, the resume and the
/// end, and reads the same state from the run. A key names one
/// message, and an ended run takes none.
#[tokio::test]
async fn messages_reach_the_runs_next_turn_and_its_host_follows_the_run() {
    let mut d = daemon(vec![
        Turn::script("tu_1", "Ask", ASK),
        Turn::script("tu_2", "Again", "return 2;"),
        Turn::finish("Cleared."),
    ])
    .await;
    let run = d
        .host
        .start_run("chat", json!({"ask": "Clear the inbox."}), None)
        .await
        .expect("the run starts");
    let first = d
        .host
        .send_message(
            &run,
            json!({"say": "Start with the oldest.", "note": "n"}),
            Some("k1"),
        )
        .await
        .expect("sent");
    assert_eq!((first.seq, first.waiting), (1, false));

    // The first turn is given it; the script asks and the run parks.
    d.tick_until_parked().await;
    d.host.serve_once().await;
    let ask = d.host.calls_of("ask");
    assert_eq!(ask.len(), 1);
    let ask_key = ask[0].key.clone();
    let turn1 = d.sent(0);
    assert_eq!(turn1.matches("Message 1 to this run").count(), 1, "{turn1}");
    assert!(turn1.contains("\"/say\"") && turn1.contains("Start with the oldest."));
    let state = d.state(&run).await;
    assert_eq!(
        (state.status.as_str(), state.steps, state.messages_waiting),
        ("parked", 1, 0)
    );
    assert_eq!(
        state.waiting_on,
        Some(json!({"surface": "inbox", "op": "ask", "deadline": "2026-11-12T00:00:00.000Z"}))
    );

    // A message to the parked run leaves it parked on the same question,
    // which no one answered and which is not sent again.
    let second = d
        .host
        .send_message(
            &run,
            json!({"say": "Skip the drafts.", "note": "n"}),
            Some("k2"),
        )
        .await
        .expect("sent");
    assert_eq!((second.seq, second.waiting), (2, true));
    assert_eq!(d.state(&run).await.status, "parked");
    d.tick().await;
    d.host.serve_once().await;
    let state = d.state(&run).await;
    assert_eq!(
        (state.status.as_str(), state.steps, state.messages_waiting),
        ("parked", 1, 1)
    );
    assert_eq!(d.host.calls_of("ask").len(), 1);
    assert_eq!(d.host.applied(&ask_key), 0);
    let call =
        d.ts.store
            .pending_call_at(&run, 2, 0)
            .unwrap()
            .expect("the question is kept");
    assert!(call.answered_at.is_none());
    assert_eq!(d.fake.requests().len(), 1, "no turn was taken meanwhile");
    // The same key and message again is the same message.
    let again = d
        .host
        .send_message(
            &run,
            json!({"say": "Skip the drafts.", "note": "n"}),
            Some("k2"),
        )
        .await
        .expect("sent again");
    assert_eq!(again, second);
    let (s, v) = d
        .host
        .send_message(&run, json!({"say": "Other.", "note": "n"}), Some("k2"))
        .await
        .unwrap_err();
    assert_eq!((s, code(&v)), (409, "key_conflict"), "{v}");

    // The host answers the question: the run goes on, and the next turn
    // is given the second message, once.
    d.host.clock().advance(1);
    d.host.serve_once().await;
    d.tick().await;
    let state = d.state(&run).await;
    assert_eq!(state.status, "finished", "{state:?}");
    let turn2 = d.sent(1);
    assert_eq!(turn2.matches("Message 1 to this run").count(), 1, "{turn2}");
    assert_eq!(turn2.matches("Message 2 to this run").count(), 1, "{turn2}");
    assert!(turn2.find("Message 1 to").unwrap() < turn2.find("Message 2 to").unwrap());
    assert_eq!(d.sent(2).matches("Message 2 to this run").count(), 1);
    let delivered: Vec<Option<i64>> =
        d.ts.store
            .messages_of(&run)
            .unwrap()
            .iter()
            .map(|m| m.delivered_step)
            .collect();
    assert_eq!(delivered, [Some(1), Some(3)]);

    // The host was told how the run went, and how it ended.
    d.host.serve_once().await;
    d.host.serve_once().await;
    assert_eq!(
        d.events(&run),
        [
            json!({"type": "step", "step": 1, "kind": "turn", "messages": [1]}),
            json!({"type": "parked", "step": 2,
                   "waiting_on": {"surface": "inbox", "op": "ask",
                                  "deadline": "2026-11-12T00:00:00.000Z"}}),
            json!({"type": "resumed", "step": 2}),
            json!({"type": "step", "step": 2, "kind": "script"}),
            json!({"type": "step", "step": 3, "kind": "turn", "messages": [2]}),
            json!({"type": "step", "step": 4, "kind": "script"}),
            json!({"type": "step", "step": 5, "kind": "turn"}),
        ]
    );
    let ended = d.host.ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end, End::Finished {});
    let state = d.state(&run).await;
    assert_eq!(
        state,
        RunState {
            run: run.clone(),
            kind: "chat".into(),
            status: "finished".into(),
            steps: 6,
            waiting_on: None,
            messages_waiting: 0,
            undelivered: vec![],
            end: Some(End::Finished {}),
            result: Some(json!({"summary": "Cleared."})),
        }
    );
    // An ended run takes no message, but answers a key it holds, while
    // it keeps the message.
    let (s, v) = d
        .host
        .send_message(&run, json!({"say": "More.", "note": "n"}), None)
        .await
        .unwrap_err();
    assert_eq!((s, code(&v)), (409, "run_ended"), "{v}");
    assert_eq!(
        d.host
            .send_message(
                &run,
                json!({"say": "Start with the oldest.", "note": "n"}),
                Some("k1")
            )
            .await
            .unwrap(),
        first
    );
}

/// A kind with one parameter takes no message, a message not of the
/// kind's type is refused, and another registration can neither send to
/// the run, nor read it, nor is told of it.
#[tokio::test]
async fn a_message_is_refused_unless_the_runs_kind_takes_it_and_its_starter_sends_it() {
    let mut d = daemon(vec![Turn::script("tu_1", "Ask", ASK)]).await;
    let once = d
        .host
        .start_run("once", json!({"ask": "Do it."}), None)
        .await
        .expect("the run starts");
    let (s, v) = d
        .host
        .send_message(&once, json!({"say": "x", "note": "n"}), None)
        .await
        .unwrap_err();
    assert_eq!((s, code(&v)), (400, "takes_no_messages"), "{v}");

    let run = d
        .host
        .start_run("chat", json!({"ask": "Clear the inbox."}), None)
        .await
        .expect("the run starts");
    for wrong in [
        json!({"say": 1, "note": "n"}),
        json!({"say": "x"}),
        json!({"say": "x", "note": "n", "extra": true}),
        json!("x"),
    ] {
        let (s, v) = d
            .host
            .send_message(&run, wrong.clone(), None)
            .await
            .unwrap_err();
        assert_eq!((s, code(&v)), (400, "invalid_request"), "{wrong}: {v}");
    }
    let (s, v) = d
        .host
        .send_message(
            "01JD5Y8Q2W3E4R5T6Y7U8I9O0P",
            json!({"say": "x", "note": "n"}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!((s, code(&v)), (404, "unknown_run"), "{v}");

    let mut other = inbox_host(&d.ts).wait_ms(0);
    other.register().await.expect("a second registration");
    let (s, v) = other
        .send_message(&run, json!({"say": "x", "note": "n"}), None)
        .await
        .unwrap_err();
    assert_eq!((s, code(&v)), (404, "unknown_run"), "{v}");
    let (s, v) = other.run_state(&run).await.unwrap_err();
    assert_eq!((s, code(&v)), (404, "unknown_run"), "{v}");
    // A message of the right type is taken from the registration that
    // started the run.
    d.host
        .send_message(&run, json!({"say": "x", "note": "n"}), None)
        .await
        .expect("sent");
    assert!(d.ts.store.messages_of(&once).unwrap().is_empty());

    // The runs go on (one takes the one scripted turn and parks, the
    // other ends); their events reach their starter and no other.
    d.tick_until_parked().await;
    d.host.serve_once().await;
    other.serve_once().await;
    assert!(!d.host.events().is_empty());
    assert!(other.events().is_empty(), "{:?}", other.events());
}

impl Daemon {
    /// The value the run's script step answered.
    fn script_value(&self, run: &str) -> Value {
        let (_, steps) = self.ts.store.run_with_steps(run).unwrap().unwrap();
        let step = steps.iter().find(|s| s.n == 2).expect("the script step");
        step.payload.as_ref().unwrap()["outcome"]["value"].clone()
    }
}

/// The host is away for longer than its lease while its run waits on
/// the question, and sends a message before it serves again: the run
/// stays parked and the question open, nothing is journaled, and when
/// the host is back and the person answers, the script is given the
/// answer and the next turn the message.
#[tokio::test]
async fn a_message_from_a_host_away_past_its_lease_leaves_the_question_open() {
    let mut d = daemon(vec![
        Turn::script("tu_1", "Ask", ASK),
        Turn::finish("Cleared."),
    ])
    .await;
    let run = d
        .host
        .start_run("chat", json!({"ask": "Clear the inbox."}), None)
        .await
        .expect("the run starts");
    d.tick_until_parked().await;
    d.host.serve_once().await;
    assert_eq!(d.host.calls_of("ask").len(), 1);
    let registration = d.host.registration().unwrap().to_string();

    // Six minutes away: the hub's lease of five is over.
    d.clock.advance_millis(6 * 60_000);
    let sent = d
        .host
        .send_message(&run, json!({"say": "Skip the drafts.", "note": "n"}), None)
        .await
        .expect("sent");
    assert_eq!((sent.seq, sent.waiting), (1, true));
    d.tick().await;
    let state = d.state(&run).await;
    assert_eq!(
        (state.status.as_str(), state.messages_waiting),
        ("parked", 1),
        "{state:?}"
    );
    assert!(state.waiting_on.is_some());
    assert!(d.ts.store.bot_calls(&run, 2).unwrap().is_empty());
    let call =
        d.ts.store
            .pending_call_at(&run, 2, 0)
            .unwrap()
            .expect("the question is kept");
    assert!(call.answered_at.is_none());

    // Back as the registration it was, the host brings the answer.
    assert_eq!(
        d.host.register().await.expect("back").registration,
        registration
    );
    d.host.clock().advance(1);
    d.host.serve_once().await;
    d.tick().await;
    let state = d.state(&run).await;
    assert_eq!(state.status, "finished", "{state:?}");
    assert_eq!(d.script_value(&run), json!("the oldest"));
    assert_eq!(d.sent(1).matches("Message 1 to this run").count(), 1);
    assert_eq!(d.host.calls_of("ask").len(), 1);
}

/// A message to a parked run that is then cancelled is never given to
/// its model: the run's `ended` and its state name it.
#[tokio::test]
async fn a_message_a_cancelled_run_never_took_is_named_in_its_end() {
    let mut d = daemon(vec![Turn::script("tu_1", "Ask", ASK)]).await;
    let run = d
        .host
        .start_run("chat", json!({"ask": "Clear the inbox."}), None)
        .await
        .expect("the run starts");
    d.tick_until_parked().await;
    d.host.serve_once().await;
    let sent = d
        .host
        .send_message(&run, json!({"say": "Stop.", "note": "n"}), None)
        .await
        .expect("sent");
    assert!(sent.waiting);
    let r =
        d.ts.client
            .post(format!("{}/api/bot/runs/{run}/cancel", d.ts.base))
            .header("cookie", d.ts.owner_cookie())
            .send()
            .await
            .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!(s, 200, "{v}");
    d.tick().await;
    d.host.serve_once().await;
    let ended = d.host.ended();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(
        (&ended[0].end, ended[0].undelivered.as_slice()),
        (
            &End::Stopped {
                reason: "cancelled".into()
            },
            &[1][..]
        )
    );
    let state = d.state(&run).await;
    assert_eq!(
        (
            state.status.as_str(),
            state.undelivered,
            state.messages_waiting
        ),
        ("stopped", vec![1], 1)
    );
    // No event of the run comes after its end.
    let events = d.events(&run);
    assert_ne!(events.last().unwrap()["kind"], "stopped", "{events:?}");
}
