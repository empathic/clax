//! A run's inbox, end to end in one process (`clax_store::bot_messages`,
//! the turn in `clax_bot::run`): messages sent to a run of a host's kind
//! that takes them are given to its model once, in the order they
//! arrived, at the start of its next turn, marked as the request marks
//! the person's words; a crash before that turn's commit gives them
//! again, and only the commit marks them delivered. A message to a run
//! parked on a call that waits leaves it parked: the call is ended only
//! by its answer, its deadline or a cancel, and the message is given at
//! the first turn after. One the run never gives (its next step is not
//! a turn) is named in its end. The host's registration is told how the
//! run goes ([`clax_bot::surface::Progress`]).
//!
//! The surface is [`INBOX`], a test's own, served by the scripted host
//! of `outside.rs` over the hub kept in the store.

use crate::common::*;
use crate::generic::set_row;
use crate::outside::{Behave, Host, Kept, World, registering, result};
use crate::resume::crash_once;
use clax_bot::policy::RunLimits;
use clax_bot::run::{CrashPoint, Deps, RunSettings};
use clax_bot::surface::RunInfo;
use clax_bot::{DefaultPolicy, Next, RunPolicy, Wait};
use clax_store::bot_messages::{Appended, MessageBounds, NewMessage};
use clax_store::bot_runs::{Bindings, DriverId, RunStatus, StepKind, StoredBinding};
use clax_surface::wire::{self, Settings};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// A surface whose kind `chat` takes messages.
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
}
"#;

const BOUNDS: MessageBounds = MessageBounds {
    undelivered: 8,
    bytes: 4096,
};

/// A script that asks the person, and answers what they said.
const ASK: &str = r#"return await clax.inbox.ask("Which first?");"#;

/// How the inbox's host answers: `items` at once, `ask` held
/// unanswered, for the person to answer later.
fn inbox_host() -> Behave {
    Box::new(|call| match call.op.as_str() {
        "items" => result(json!(["a", "b"])),
        _ => None,
    })
}

struct Inbox {
    env: Env,
    world: World,
    host: Arc<Host>,
}

impl Inbox {
    async fn new() -> Inbox {
        let env = Env::new().await;
        let world = World::new(&env, Kept::InTheStore);
        let request = registering(INBOX, None, None, Settings::default());
        let host = world.host("inbox", &request, inbox_host());
        Inbox { env, world, host }
    }

    fn deps(&self) -> Deps {
        self.deps_of(&self.world, Arc::new(DefaultPolicy))
    }

    /// Deps whose bindings are served through `world`'s hub, under
    /// `policy`.
    fn deps_of(&self, world: &World, policy: Arc<dyn RunPolicy>) -> Deps {
        let mut deps = self.env.deps();
        deps.policy = policy.clone();
        deps.providers.set_resolver(world.resolver(policy));
        deps
    }

    /// A run of `inbox.chat` its host's registration started.
    fn run(&self) -> String {
        let binding = StoredBinding::bound(
            "inbox",
            None,
            Some(self.host.registration.clone()),
            None,
            Value::Null,
            vec!["inbox.items".into(), "inbox.ask".into()],
        );
        let bindings = Bindings::bound(vec![binding]).unwrap();
        let run = self.env.run();
        set_row(
            &self.env,
            &run,
            &format!(
                "kind = 'inbox.chat', artifact_id = NULL, base_version = NULL, \
                 bindings_json = ?2, request_json = ?3, started_by = '{}'",
                self.host.registration
            ),
            &bindings.to_json(),
            &json!({"ask": "Clear the inbox."}).to_string(),
        );
        run
    }

    fn send(&self, run: &str, body: Value) -> i64 {
        self.sent(run, body).message.seq
    }

    fn sent(&self, run: &str, body: Value) -> Appended {
        self.env
            .store
            .append_message(
                NewMessage {
                    run_id: run,
                    registration_id: &self.host.registration,
                    body: &body,
                    key: None,
                },
                BOUNDS,
            )
            .unwrap()
    }

    /// The `ask` call of `run` the host was sent.
    fn asked(&self, run: &str) -> String {
        let st = self.host.state.lock().unwrap();
        let ask = st.calls.iter().find(|c| c.op == "ask" && c.run == run);
        ask.expect("the host was asked").id.clone()
    }

    /// The host, back after however long as the registration it was,
    /// answers `run`'s question.
    fn answer(&self, run: &str, answer: &str) {
        self.host.online.store(true, Ordering::SeqCst);
        self.host.back(INBOX, 900);
        self.host
            .answer(&self.asked(run), wire::Outcome::Result(json!(answer)));
    }

    /// Ticks the run until it ends or waits.
    async fn drive(&self, deps: &Deps, run: &str) -> Next {
        let out = self.env.drive(deps, run, &DriverId::mint()).await;
        out.last().unwrap().next
    }

    /// The value step `n` (from 0) answered.
    fn value(&self, run: &str, n: usize) -> Value {
        let steps = self.env.store.run_with_steps(run).unwrap().unwrap().1;
        steps[n].payload.as_ref().unwrap()["outcome"]["value"].clone()
    }

    /// The call journal of step `n`, as `(op, ok)`.
    fn journal(&self, run: &str, n: i64) -> Vec<(String, bool)> {
        let calls = self.env.store.bot_calls(run, n).unwrap();
        calls.iter().map(|c| (c.op().to_string(), c.ok())).collect()
    }

    fn delivered_steps(&self, run: &str) -> Vec<Option<i64>> {
        let all = self.env.store.messages_of(run).unwrap();
        all.iter().map(|m| m.delivered_step).collect()
    }

    /// The text of the user blocks of model request `n`, in order.
    fn sent_texts(&self, n: usize) -> Vec<String> {
        let body = &self.env.api.requests()[n].body;
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "user")
            .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
            .filter(|b| b["type"] == "text")
            .map(|b| b["text"].as_str().unwrap().to_string())
            .collect()
    }
}

fn count(texts: &[String], needle: &str) -> usize {
    texts.iter().filter(|t| t.contains(needle)).count()
}

/// Two messages sent before the run's first turn are given to it once,
/// after the request and in the order they arrived, each with the
/// places of the person's words in it; the next turn sees them once, as
/// history, and the committed turn records which it delivered.
#[tokio::test]
async fn messages_are_given_to_the_next_turn_once_in_the_order_they_arrived() {
    let d = Inbox::new().await;
    d.env.api.push(script_turn("tu_1", "Items", "return 1;"));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    assert_eq!(
        d.send(
            &run,
            json!({"say": "Start with the oldest.", "note": "from the bar"})
        ),
        1
    );
    assert_eq!(
        d.send(&run, json!({"say": "Skip the drafts.", "note": "n"})),
        2
    );
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));

    let first = d.sent_texts(0);
    assert_eq!(first.len(), 3, "{first:#?}");
    assert!(first[0].contains("Clear the inbox."), "{}", first[0]);
    assert!(
        first[1].starts_with("Message 1 to this run, sent after it began.")
            && first[1].contains("\"/say\"\n")
            && first[1].contains("Start with the oldest.")
            && !first[1].contains("\"/note\""),
        "{}",
        first[1]
    );
    assert!(first[2].starts_with("Message 2 to this run") && first[2].contains("Skip the drafts."));
    // The next turn sees each once, where it was given.
    let second = d.sent_texts(1);
    assert_eq!(count(&second, "Message 1 to this run"), 1);
    assert_eq!(count(&second, "Message 2 to this run"), 1);
    // The system prompt says what a message is.
    let system = d.env.api.requests()[0].body["system"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(system.contains("A message to the run may come after the request"));

    let all = d.env.store.messages_of(&run).unwrap();
    assert_eq!(
        all.iter().map(|m| m.delivered_step).collect::<Vec<_>>(),
        [Some(1), Some(1)]
    );
    assert!(d.env.store.undelivered_messages(&run).unwrap().is_empty());
}

/// A message sent while the run is between steps is given at the turn
/// after its script, not before.
#[tokio::test]
async fn a_message_sent_after_a_turn_waits_for_the_next_one() {
    let d = Inbox::new().await;
    d.env.api.push(script_turn(
        "tu_1",
        "Items",
        "return await clax.inbox.items();",
    ));
    d.env.api.push(script_turn("tu_2", "Again", "return 2;"));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    let deps = d.deps();
    let driver = DriverId::mint();
    // The first turn, then the script.
    assert_eq!(
        d.env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    d.send(&run, json!({"say": "Now the archive.", "note": "n"}));
    assert_eq!(
        d.env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Script)
    );
    let out = d.env.drive(&deps, &run, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(count(&d.sent_texts(0), "Message 1 to this run"), 0);
    let second = d.sent_texts(1);
    assert!(
        second.last().unwrap().starts_with("Message 1 to this run"),
        "{second:#?}"
    );
    assert_eq!(count(&d.sent_texts(2), "Message 1 to this run"), 1);
    assert_eq!(
        d.env.store.messages_of(&run).unwrap()[0].delivered_step,
        Some(3)
    );

    // The host was told of each step, the turn that gave the message
    // with its `seq`.
    d.host.round();
    let events: Vec<Value> = d
        .host
        .state
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|e| e.run == run)
        .map(|e| e.event.clone())
        .collect();
    assert_eq!(
        events,
        [
            json!({"type": "step", "step": 1, "kind": "turn"}),
            json!({"type": "step", "step": 2, "kind": "script"}),
            json!({"type": "step", "step": 3, "kind": "turn", "messages": [1]}),
            json!({"type": "step", "step": 4, "kind": "script"}),
            json!({"type": "step", "step": 5, "kind": "turn"}),
        ]
    );
}

/// The driver dies before the commit of the turn that was given a
/// message: the message is still waiting, the turn run again is given
/// it again, and its commit alone marks it delivered, so the model's
/// history holds it once.
#[tokio::test]
async fn a_crash_before_the_turns_commit_gives_the_message_again_exactly_once() {
    let d = Inbox::new().await;
    // The crashed turn is asked again.
    d.env.api.push(script_turn("tu_1", "Items", "return 1;"));
    d.env.api.push(script_turn("tu_1", "Items", "return 1;"));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    d.send(&run, json!({"say": "Mind the flagged ones.", "note": "n"}));
    let mut dying = d.deps();
    crash_once(
        &mut dying,
        CrashPoint::BeforeCommit {
            step: 1,
            kind: StepKind::Turn,
        },
    );
    let out = d.env.drive(&dying, &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Crashed));
    assert_eq!(d.env.store.undelivered_messages(&run).unwrap().len(), 1);
    assert_eq!(count(&d.sent_texts(0), "Message 1 to this run"), 1);

    d.env.clock.advance(61);
    let out = d.env.drive(&d.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(count(&d.sent_texts(1), "Message 1 to this run"), 1);
    assert_eq!(count(&d.sent_texts(2), "Message 1 to this run"), 1);
    let steps = d.env.store.run_with_steps(&run).unwrap().unwrap().1;
    assert_eq!(steps[0].attempt, 2);
    let messages = d.env.store.messages_of(&run).unwrap();
    assert_eq!(messages[0].delivered_step, Some(1));
}

/// At most one run of the person's parked.
struct OneParked;

impl RunPolicy for OneParked {
    fn limits(&self, _run: &RunInfo, settings: &RunSettings) -> clax_core::Result<RunLimits> {
        let mut limits = RunLimits::of(settings);
        limits.max_parked_runs = Some(1);
        Ok(limits)
    }
}

/// The run's host is away for longer than its lease while the run waits
/// on its question, so the hub no longer holds its registration, and a
/// message comes: the run stays parked, the question stays open with
/// its host, and the person's answer, when it comes, is the one the
/// script is given. The message is given at the turn after.
#[tokio::test]
async fn a_message_to_a_run_whose_host_is_away_leaves_its_question_open() {
    let d = Inbox::new().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    let deps = d.deps();
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    let parked = d.env.run_row(&run);
    let parks = d.env.store.run_parking(&run).unwrap().unwrap().parks;
    // Away for an hour: the hub's lease of five minutes is over.
    d.host.online.store(false, Ordering::SeqCst);
    d.world.pass(&d.env, 3600);
    let sent = d.sent(&run, json!({"say": "Skip the drafts.", "note": "n"}));
    assert!(sent.new && sent.waiting);
    assert_eq!(d.env.run_row(&run), parked);
    // Ticked, the run is left where it is.
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    assert_eq!(d.env.run_row(&run).status, RunStatus::Parked);
    assert_eq!(d.env.store.run_parking(&run).unwrap().unwrap().parks, parks);
    assert!(d.journal(&run, 2).is_empty(), "nothing is journaled");
    let call = d.env.store.pending_call_at(&run, 2, 0).unwrap().unwrap();
    assert!(call.answered_at.is_none());

    // The person answers, and the script is given what they said.
    d.answer(&run, "the oldest");
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!("the oldest"));
    assert_eq!(d.journal(&run, 2), [("inbox.ask".to_string(), true)]);
    assert_eq!(d.delivered_steps(&run), [Some(3)]);
    assert_eq!(count(&d.sent_texts(1), "Message 1 to this run"), 1);
    assert_eq!(
        d.host
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.op == "ask")
            .count(),
        1,
        "the question was sent once"
    );
}

/// The daemon restarts while the run waits on its question, and a
/// message comes before its host has registered again: the new hub
/// holds no registration, and the run stays parked. The host comes back
/// as the registration it was and answers; the script is given the
/// answer.
#[tokio::test]
async fn a_message_after_a_restart_before_the_host_is_back_leaves_its_question_open() {
    let d = Inbox::new().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    assert_eq!(d.drive(&d.deps(), &run).await, Next::Waiting(Wait::Parked));
    let asked = d.asked(&run);

    // The restarted daemon's hub, over the same store, holds nothing.
    let again = World::new(&d.env, Kept::InTheStore);
    let deps = d.deps_of(&again, Arc::new(DefaultPolicy));
    assert!(
        d.sent(&run, json!({"say": "Skip the drafts.", "note": "n"}))
            .waiting
    );
    assert_eq!(d.drive(&deps, &run).await, Next::Waiting(Wait::Parked));
    let call = d.env.store.pending_call_at(&run, 2, 0).unwrap().unwrap();
    assert!(call.answered_at.is_none());

    // The host is back as the registration it was, and answers.
    let request = registering(
        INBOX,
        Some(d.host.registration.clone()),
        None,
        Settings::default(),
    );
    let back = again.host("inbox", &request, inbox_host());
    assert_eq!(back.registration, d.host.registration);
    back.answer(&asked, wire::Outcome::Result(json!("the oldest")));
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!("the oldest"));
    assert_eq!(d.journal(&run, 2), [("inbox.ask".to_string(), true)]);
    assert_eq!(d.delivered_steps(&run), [Some(3)]);
}

/// A message to a parked run keeps its place among its person's parked
/// runs: another run of the person's that would park is refused the
/// wait, and the first run's question stays open and is answered.
#[tokio::test]
async fn a_message_to_a_parked_run_keeps_its_place_among_its_persons_parked_runs() {
    let d = Inbox::new().await;
    let deps = d.deps_of(&d.world, Arc::new(OneParked));
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    let first = d.run();
    assert_eq!(d.drive(&deps, &first).await, Next::Waiting(Wait::Parked));
    assert!(
        d.sent(&first, json!({"say": "Skip the drafts.", "note": "n"}))
            .waiting
    );

    let ask = "try { return await clax.inbox.ask(\"Well?\"); } catch (e) { return e.code; }";
    d.env.api.push(script_turn("tu_1", "Ask", ask));
    d.env.api.push(finish_turn("done"));
    let second = d.run();
    assert_eq!(
        d.drive(&deps, &second).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.value(&second, 1), json!("parked_runs_limit"));
    assert_eq!(d.env.run_row(&first).status, RunStatus::Parked);
    assert!(d.journal(&first, 2).is_empty());

    d.env.api.push(finish_turn("Cleared."));
    d.answer(&first, "the oldest");
    assert_eq!(
        d.drive(&deps, &first).await,
        Next::Done(RunStatus::Finished)
    );
    assert_eq!(d.value(&first, 1), json!("the oldest"));
    assert_eq!(d.journal(&first, 2), [("inbox.ask".to_string(), true)]);
    assert_eq!(d.delivered_steps(&first), [Some(3)]);
}

/// Messages sent before the run parks, while it is parked and after its
/// question is answered are all given at the first turn after the wait,
/// once each, in the order they arrived.
#[tokio::test]
async fn messages_sent_around_a_wait_are_given_at_the_first_turn_after_it_in_order() {
    let d = Inbox::new().await;
    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    d.env.api.push(finish_turn("Cleared."));
    let run = d.run();
    let deps = d.deps();
    let driver = DriverId::mint();
    assert_eq!(
        d.env.tick(&deps, &run, &driver).await.kind,
        Some(StepKind::Turn)
    );
    let before = d.sent(&run, json!({"say": "Before.", "note": "n"}));
    assert!(!before.waiting);
    let out = d.env.drive(&deps, &run, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Waiting(Wait::Parked));
    let during = d.sent(&run, json!({"say": "During.", "note": "n"}));
    assert!(during.waiting);
    d.answer(&run, "the oldest");
    let after = d.sent(&run, json!({"say": "After.", "note": "n"}));
    assert!(!after.waiting);
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));

    assert_eq!(count(&d.sent_texts(0), "to this run"), 0);
    let turn = d.sent_texts(1);
    let at = |needle: &str| {
        assert_eq!(count(&turn, needle), 1, "{turn:#?}");
        turn.iter().position(|t| t.contains(needle)).unwrap()
    };
    let (one, two, three) = (at("Before."), at("During."), at("After."));
    assert!(one < two && two < three, "{turn:#?}");
    assert!(turn[one].starts_with("Message 1 to this run"));
    assert!(turn[three].starts_with("Message 3 to this run"));
    assert_eq!(d.delivered_steps(&run), [Some(3), Some(3), Some(3)]);
    assert_eq!(d.value(&run, 1), json!("the oldest"));
}

/// A message the run never gives its model, because its next step is
/// not a turn, is named in the run's end: one sent after the turn that
/// asked to finish, and one sent to a parked run that is cancelled.
#[tokio::test]
async fn a_message_never_given_is_named_in_the_runs_end() {
    let d = Inbox::new().await;
    d.env.api.push(finish_turn("Cleared."));
    let finished = d.run();
    let deps = d.deps();
    let driver = DriverId::mint();
    assert_eq!(
        d.env.tick(&deps, &finished, &driver).await.kind,
        Some(StepKind::Turn)
    );
    d.send(&finished, json!({"say": "Too late.", "note": "n"}));
    let out = d.env.drive(&deps, &finished, &driver).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));

    d.env.api.push(script_turn("tu_1", "Ask", ASK));
    let stopped = d.run();
    assert_eq!(d.drive(&deps, &stopped).await, Next::Waiting(Wait::Parked));
    d.send(&stopped, json!({"say": "One.", "note": "n"}));
    d.send(&stopped, json!({"say": "Two.", "note": "n"}));
    d.env
        .store
        .cancel_run(&clax_bot::run::bot_ctx(clax_store::roles::OWNER), &stopped)
        .unwrap();
    assert_eq!(
        d.drive(&deps, &stopped).await,
        Next::Done(RunStatus::Stopped)
    );

    d.host.round();
    let st = d.host.state.lock().unwrap();
    let ends: Vec<(&str, &wire::End, &[u64])> = st
        .ended
        .iter()
        .map(|e| (e.run.as_str(), &e.end, e.undelivered.as_slice()))
        .collect();
    assert_eq!(
        ends,
        [
            (finished.as_str(), &wire::End::Finished {}, &[1][..]),
            (
                stopped.as_str(),
                &wire::End::Stopped {
                    reason: "cancelled".into()
                },
                &[1, 2][..]
            ),
        ]
    );
    // No event of a run comes after its end, and the step that ended it
    // is told by the end alone.
    let finished_events: Vec<Value> = st
        .events
        .iter()
        .filter(|e| e.run == finished)
        .map(|e| e.event.clone())
        .collect();
    assert_eq!(
        finished_events,
        [json!({"type": "step", "step": 1, "kind": "turn"})]
    );
}

/// A call its host fails, saying it may be tried again, and is sent
/// again within its step tells the host of nothing more: each step the
/// run commits is one `step` event, however often a call of it was
/// sent, and every event of the run comes before its end.
#[tokio::test]
async fn a_call_sent_again_tells_its_host_of_each_step_once_and_before_its_end() {
    let env = Env::new().await;
    let world = World::new(&env, Kept::InTheStore);
    let request = registering(INBOX, None, None, Settings::default());
    let mut first = true;
    let flaky: Behave = Box::new(move |call| match call.op.as_str() {
        "items" if std::mem::take(&mut first) => Some(wire::Outcome::Error(wire::CallError::new(
            clax_bot::registry::CODE_UNAVAILABLE,
            "the inbox is busy",
        ))),
        "items" => result(json!(["a", "b"])),
        _ => None,
    });
    let host = world.host("inbox", &request, flaky);
    let d = Inbox { env, world, host };
    d.world.broker.poll_ms.store(500, Ordering::SeqCst);
    d.env.api.push(script_turn(
        "tu_1",
        "Items",
        "return await clax.inbox.items();",
    ));
    d.env.api.push(finish_turn("done"));
    let run = d.run();
    let deps = d.deps_of(&d.world, crate::outside::retrying(3));
    assert_eq!(d.drive(&deps, &run).await, Next::Done(RunStatus::Finished));
    assert_eq!(d.value(&run, 1), json!(["a", "b"]));

    d.host.round();
    let st = d.host.state.lock().unwrap();
    let items: Vec<&wire::Call> = st.calls.iter().filter(|c| c.op == "items").collect();
    assert_eq!(items.len(), 2, "sent again once");
    assert_eq!(items[0].id, items[1].id);
    let events: Vec<Value> = st
        .events
        .iter()
        .filter(|e| e.run == run)
        .map(|e| e.event.clone())
        .collect();
    assert_eq!(
        events,
        [
            json!({"type": "step", "step": 1, "kind": "turn"}),
            json!({"type": "step", "step": 2, "kind": "script"}),
            json!({"type": "step", "step": 3, "kind": "turn"}),
        ]
    );
    let told: Vec<&str> = st
        .told
        .iter()
        .filter(|(of, _)| *of == run)
        .map(|(_, what)| *what)
        .collect();
    assert_eq!(told, ["event", "event", "event", "ended"]);
}

/// A run of Clax's own kinds takes no messages: one in its inbox (put
/// there by hand, as no route lets a host message such a run) is never
/// read, given to the model, or marked delivered.
#[tokio::test]
async fn a_clax_run_never_reads_its_inbox() {
    let env = Env::new().await;
    env.api.push(finish_turn("done"));
    let run = env.run();
    set_row(&env, &run, "started_by = ?2 || ?3", "r1", "");
    env.store
        .append_message(
            NewMessage {
                run_id: &run,
                registration_id: "r1",
                body: &json!({"say": "PLANTED-IN-THE-INBOX"}),
                key: None,
            },
            BOUNDS,
        )
        .unwrap();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let sent: Vec<Seen> = env.api.requests();
    assert!(!sent.is_empty());
    assert!(
        sent.iter().all(|r| !r.raw.contains("PLANTED-IN-THE-INBOX")),
        "a clax run's model was given its inbox"
    );
    let kept = env.store.messages_of(&run).unwrap();
    assert!(kept[0].delivered_at.is_none(), "{kept:?}");
}
