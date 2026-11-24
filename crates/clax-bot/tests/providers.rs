//! The fake surface provider: it passes the cases every provider passes
//! (`clax_bot::provider_conformance`), and it is a fake that tells a
//! test what a run loop did with it. Clax's own provider
//! (`clax_bot::clax::ClaxProvider`) passes the same cases.

use crate::common::*;
use clax_bot::clax::ClaxProvider;
use clax_bot::host::NOTICE_SEQ;
use clax_bot::provider_conformance::{
    Maybe, OpCall, ProviderFixture, Staged, Untagged, Unwritten, Waiting, check_provider,
};
use clax_bot::surface::{
    Answer, Binding, Bound, Call, Claim, End, FakeSurfaceProvider, Fields, Hook, Journal,
    NOTICE_STRIDE, RunInfo, StepCx, StepEnd, StepSession, SurfaceProvider, dispatch, perform,
    recorded,
};
use clax_bot::web::{SearchAnswer, WebSource};
use clax_bot_sandbox::{CancelToken, HostError};
use clax_store::bot_runs::{DriverId, RunStatus, Step, StepKind};
use clax_store::bot_settings::Setting;
use clax_surface::Definition;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What the core would know of a run of the fake's kind whose row is
/// run `id` of the tests' store: the row is a `clax` run's, which
/// stands for it until the store keeps a run with no artifact.
fn fake_run(id: &str) -> RunInfo {
    RunInfo {
        id: id.to_string(),
        person: clax_store::roles::OWNER.to_string(),
        model: "claude-sonnet-5-5".to_string(),
        kind: "fake.job".to_string(),
        request: json!({"ask": "Count."}),
        bindings: vec![FakeSurfaceProvider::binding()],
    }
}

/// The fake's fixture for the provider cases, over three new runs of
/// `env`.
fn fake_fixture(env: &Env, fake: &Arc<FakeSurfaceProvider>) -> ProviderFixture {
    let run = fake_run(&env.run());
    let (cancelled_run, ending_run) = (fake_run(&env.run()), fake_run(&env.run()));
    // Each write the fake applied, by key: a write under a new key adds
    // one.
    let state = {
        let fake = fake.clone();
        move || {
            let st = fake.lock();
            let sets: Vec<Value> = st
                .applied
                .iter()
                .filter(|a| a.op == "fake.set")
                .map(|a| json!([a.key, a.args]))
                .collect();
            Ok(json!(sets))
        }
    };
    let noticed = {
        let fake = fake.clone();
        move || {
            let st = fake.lock();
            let notices: Vec<&Value> = st
                .applied
                .iter()
                .filter(|a| a.op == "notice")
                .map(|a| &a.answer)
                .collect();
            Ok(json!(notices))
        }
    };
    let lifecycle = {
        let fake = fake.clone();
        move |run: &str| fake.lifecycle(run)
    };
    let fail_once = {
        let fake = fake.clone();
        move |hook: Hook| {
            fake.lock().fail_hooks.push(hook);
            Ok(())
        }
    };
    let unwritten = {
        let fake = fake.clone();
        move || {
            let busy = HostError::new("busy", "the fake is busy").retryable();
            fake.lock().unwritten_next.insert("fake.get".into(), busy);
        }
    };
    let applied = {
        let fake = fake.clone();
        move |key: &str| Ok(fake.lock().applied.iter().filter(|a| a.key == key).count() as u64)
    };
    let untagged = {
        let fake = fake.clone();
        move || fake.lock().wait_next.push("fake.get".into())
    };
    ProviderFixture {
        provider: fake.clone(),
        store: env.store.clone(),
        run,
        read: OpCall::new("fake.count", json!([])),
        disturb: Maybe::Lacks("the fake's count moves each time it is performed"),
        write: OpCall::new("fake.set", json!(["colour", "blue"])),
        second_write: OpCall::new("fake.set", json!(["size", "large"])),
        state: Box::new(state),
        refused: OpCall::new("fake.refuse", json!(["secret"])),
        outside: OpCall::new("fake.hidden", json!(["x"])),
        cancelled_run,
        staged: Maybe::Lacks("the fake stages nothing for its step's commit"),
        ending_run,
        end: End::Stopped {
            reason: "run_cap".into(),
            limit: Some("40 steps".into()),
            credits_reset: None,
        },
        result: None,
        noticed: Box::new(noticed),
        unwritten: Maybe::Has(Unwritten {
            call: OpCall::new("fake.get", json!(["colour"])),
            arm: Box::new(unwritten),
        }),
        waiting: Maybe::Has(Waiting {
            call: OpCall::new("fake.ask", json!(["Which colour?"])),
            answer: json!("blue"),
            applied: Box::new(applied),
        }),
        untagged: Maybe::Has(Untagged {
            call: OpCall::new("fake.get", json!(["size"])),
            arm: Box::new(untagged),
        }),
        fail_once: Maybe::Has(Box::new(fail_once)),
        lifecycle: Maybe::Has(Box::new(lifecycle)),
    }
}

/// The one case the fake is not played through: it has no write that
/// waits for its step's commit.
const STAGED: &str = "a_cancelled_step_keeps_no_write_that_waited_for_its_commit";

#[tokio::test]
async fn the_fake_provider_passes_the_provider_conformance_suite() {
    let env = Env::new().await;
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let fixture = fake_fixture(&env, &fake);
    let (cancelled, ending) = (
        fixture.cancelled_run.id.clone(),
        fixture.ending_run.id.clone(),
    );
    let results = check_provider(&fixture, &[STAGED]).await;
    assert_eq!(results.len(), 17);
    for (name, result) in results {
        result.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    let st = fake.lock();
    // The operation outside the binding reached a session only where a
    // case handed it over itself, past the core.
    let hidden: Vec<bool> = st
        .calls
        .iter()
        .filter(|c| c.op == FakeSurfaceProvider::HIDDEN)
        .map(|c| c.replayed)
        .collect();
    assert_eq!(hidden, [false], "the one bypass");
    // Each write was applied once, however many attempts made it, and
    // each notice once, though one `ending` failed after it wrote and
    // every `ending` was made again.
    let applied: Vec<(&str, &str)> = st
        .applied
        .iter()
        .map(|a| (a.op.as_str(), a.args[0].as_str().unwrap_or("")))
        .collect();
    assert_eq!(
        applied,
        [
            ("fake.set", "colour"),
            // The question that waited, kept once though its call was
            // made three times.
            ("fake.ask", "Which colour?"),
            ("fake.set", "size"),
            ("fake.set", "colour"),
            ("notice", "stopped: cancelled"),
            ("notice", "stopped: run_cap at 40 steps")
        ]
    );
    assert!(st.calls.iter().filter(|c| c.op == "fake.set").count() > 2);
    // The refusal reached the session each time it was not answered
    // from the journal, and changed nothing.
    assert!(st.calls.iter().filter(|c| c.op == "fake.refuse").count() >= 2);
    assert!(st.fail_hooks.is_empty(), "both failures were played");
    // `ended` was told twice of the run the cases ended, and the fake
    // acted on each end once.
    let told = |run: &str| {
        st.hooks
            .iter()
            .filter(|h| h.hook == Hook::Ended && h.run == run)
            .count()
    };
    assert_eq!((told(&cancelled), told(&ending)), (1, 2));
    assert_eq!(st.followed, [cancelled.as_str(), ending.as_str()]);
    // The `ending` of the run the cases ended was called in four
    // attempts of its step: the one that failed, and three more.
    let attempts: Vec<i64> = st
        .hooks
        .iter()
        .filter(|h| h.hook == Hook::Ending && h.run == ending)
        .map(|h| h.step.unwrap().1)
        .collect();
    assert_eq!(attempts, [1, 2, 3, 4]);
    drop(st);
    // The parts were committed under the surface's name, the core's own
    // field beside them.
    let payload = |run: &str, n: usize| {
        let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
        steps[n].payload.clone().unwrap()
    };
    assert_eq!(
        payload(&cancelled, 0),
        json!({"reason": "cancelled",
               "fake": {"fake_set": ["colour"], "fake_notice": "stopped: cancelled"}})
    );
    assert_eq!(
        payload(&ending, 0),
        json!({"reason": "run_cap", "fake": {"fake_notice": "stopped: run_cap at 40 steps"}})
    );
}

/// One attempt of step `n` of `run` for the fake, by `driver`.
struct Fixture {
    env: Env,
    fake: FakeSurfaceProvider,
    run: RunInfo,
    driver: DriverId,
    cancel: CancelToken,
}

impl Fixture {
    async fn new() -> Fixture {
        let env = Env::new().await;
        let run = fake_run(&env.run());
        let driver = DriverId::mint();
        env.store.claim_run(&run.id, &driver, 60).unwrap().unwrap();
        Fixture {
            fake: FakeSurfaceProvider::new(env.store.clone()),
            env,
            run,
            driver,
            cancel: CancelToken::new(),
        }
    }

    fn cx(&self, kind: StepKind) -> StepCx<'_> {
        let step = self
            .env
            .store
            .begin_step(&self.run.id, &self.driver, 1, kind)
            .unwrap();
        StepCx::new(
            &self.run,
            &self.run.bindings[0],
            1,
            step.attempt,
            &[],
            Claim::new(&self.env.store, &self.driver, &self.run.id),
            &self.cancel,
        )
    }
}

#[tokio::test]
async fn the_fake_fails_refuses_and_answers_a_repeated_key_once() {
    let f = Fixture::new().await;
    let cx = f.cx(StepKind::Script);
    let session = f.fake.begin(cx).await.unwrap();
    let call = |seq: i64, op: &'static str, args: Value| {
        let session = &session;
        let cx = &cx;
        let fake = &f.fake;
        async move {
            let place = cx.place(seq).unwrap();
            let args = args.as_array().cloned().unwrap();
            let raw = Value::Array(args.clone());
            let op = format!("fake.{op}");
            let call = Call {
                place: &place,
                op: &op,
                args: &args,
                raw: &raw,
                replay: None,
            };
            perform(fake, &**session, cx.binding, &cx.claim, call).await
        }
    };
    let failed = call(0, "fail", json!([])).await;
    assert_eq!(
        failed.result(),
        &Err(HostError::new("broken", "the fake is broken").retryable())
    );
    let refused = call(1, "refuse", json!(["x"])).await;
    assert_eq!(refused.result().as_ref().unwrap_err().code, "forbidden");
    assert_eq!(refused.note().target(), Some("x"));
    let first = call(2, "set", json!(["a", "1"])).await;
    assert_eq!(first.result(), &Ok(json!({"name": "a", "writes": 1})));
    assert_eq!(first.journal(), Journal::Answer);
    assert_eq!(call(2, "set", json!(["a", "1"])).await, first);
    let reused = call(2, "set", json!(["b", "2"])).await;
    assert_eq!(reused.into_result().unwrap_err().code, "key_reused");
    f.fake
        .lock()
        .fail_next
        .insert("fake.get".into(), HostError::new("gone", "for once"));
    let code = |a: Answer| a.into_result().unwrap_err().code;
    assert_eq!(code(call(3, "get", json!(["a"])).await), "gone");
    assert_eq!(call(4, "get", json!(["a"])).await.result(), &Ok(json!("1")));
    assert_eq!(
        call(5, "get", json!(["b"])).await.result(),
        &Ok(Value::Null)
    );
    assert_eq!(code(call(6, "hidden", json!(["x"])).await), "unknown_op");

    // The core journaled every answer, a failure with its flag, and the
    // first answer of the place the three writes shared.
    let journal = f.env.store.bot_calls(&f.run.id, 1).unwrap();
    let rows: Vec<(i64, &str, bool)> = journal.iter().map(|e| (e.seq(), e.op(), e.ok())).collect();
    assert_eq!(
        rows,
        [
            (0, "fake.fail", false),
            (1, "fake.refuse", false),
            (2, "fake.set", true),
            (3, "fake.get", false),
            (4, "fake.get", true),
            (5, "fake.get", true),
            (6, "fake.hidden", false),
        ]
    );
    assert_eq!(&recorded(&journal[0]), failed.result());
    assert_eq!(&recorded(&journal[2]), first.result());
    assert_eq!(
        recorded(&journal[6]).unwrap_err().message,
        "clax.fake.hidden is not an operation this run has"
    );

    let st = f.fake.lock();
    assert_eq!(st.applied.len(), 1);
    let seen: Vec<(&str, &str)> = st
        .calls
        .iter()
        .map(|c| (c.key.rsplit(':').next().unwrap(), c.op.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("0", "fake.fail"),
            ("1", "fake.refuse"),
            ("2", "fake.set"),
            ("2", "fake.set"),
            ("2", "fake.set"),
            ("3", "fake.get"),
            ("4", "fake.get"),
            ("5", "fake.get"),
        ],
        "every call with its key, and none for the operation outside the binding"
    );
    drop(st);
    let parts = session.finish(StepEnd::Commit);
    assert_eq!(json!(parts.payload), json!({"fake_set": ["a", "a"]}));
}

/// A call is routed to its binding, its arguments are checked against
/// that surface's definition, and only then is the binding asked
/// whether it has the operation: the order Clax's own surface always
/// had, where a web operation the run did not declare answers
/// `invalid_args` when its arguments do not hold.
#[tokio::test]
async fn a_call_is_routed_then_checked_then_asked_of_its_binding() {
    let f = Fixture::new().await;
    let cx = f.cx(StepKind::Script);
    let session = f.fake.begin(cx).await.unwrap();
    let bound = [Bound {
        binding: cx.binding,
        provider: &f.fake,
        session: &*session,
    }];
    let made = |seq: i64, op: &'static str, raw: Value| {
        let (bound, cx) = (&bound, &cx);
        async move {
            let place = cx.place(seq).unwrap();
            let answer = dispatch(bound, &cx.claim, &place, op, &raw, None).await;
            assert_eq!(
                answer.journal(),
                Journal::Outcome,
                "{op}: the core's refusal"
            );
            assert_eq!(answer.note().target(), None);
            let e = answer.into_result().unwrap_err();
            (e.code, e.message)
        }
    };
    // No binding takes it: the run has no `clax` binding, and no surface
    // named `shell`.
    assert_eq!(
        made(0, "files.read", json!(["index.html"])).await,
        (
            "unknown_op".into(),
            "clax.files.read is not an operation".into()
        )
    );
    assert_eq!(made(1, "shell.windows", json!([])).await.0, "unknown_op");
    // Routed to the fake, whose definition has no such operation.
    assert_eq!(
        made(2, "fake.nope", json!([])).await,
        (
            "unknown_op".into(),
            "clax.fake.nope is not an operation".into()
        )
    );
    // The arguments are checked before the binding is asked: an
    // operation the binding does not have, with arguments that do not
    // hold, is refused for its arguments.
    assert_eq!(made(3, "fake.hidden", json!([7])).await.0, "invalid_args");
    assert_eq!(made(4, "fake.set", json!(["a"])).await.0, "invalid_args");
    // With arguments that hold it is refused for the binding.
    assert_eq!(
        made(5, "fake.hidden", json!(["x"])).await,
        (
            "unknown_op".into(),
            "clax.fake.hidden is not an operation this run has".into()
        )
    );
    // And an operation the binding has reaches the session.
    let set = dispatch(
        &bound,
        &cx.claim,
        &cx.place(6).unwrap(),
        "fake.set",
        &json!(["a", "1"]),
        None,
    )
    .await;
    assert_eq!(set.result(), &Ok(json!({"name": "a", "writes": 1})));

    // The core journaled each refusal, in its own form, and the session
    // saw the one call that reached it.
    let journal = f.env.store.bot_calls(&f.run.id, 1).unwrap();
    let rows: Vec<(i64, &str, bool)> = journal.iter().map(|e| (e.seq(), e.op(), e.ok())).collect();
    assert_eq!(
        rows,
        [
            (0, "files.read", false),
            (1, "shell.windows", false),
            (2, "fake.nope", false),
            (3, "fake.hidden", false),
            (4, "fake.set", false),
            (5, "fake.hidden", false),
            (6, "fake.set", true),
        ]
    );
    assert_eq!(
        journal[0].result(),
        &json!({"code": "unknown_op", "message": "clax.files.read is not an operation"})
    );
    let seen: Vec<String> = f.fake.lock().calls.iter().map(|c| c.op.clone()).collect();
    assert_eq!(seen, ["fake.set"]);

    // A refusal whose place is in the journal is answered again and not
    // written again.
    let again = dispatch(
        &bound,
        &cx.claim,
        &cx.place(0).unwrap(),
        "files.read",
        &json!(["index.html"]),
        Some(&journal[0]),
    )
    .await;
    assert_eq!(again.into_result(), recorded(&journal[0]));
    assert_eq!(f.env.store.bot_calls(&f.run.id, 1).unwrap().len(), 7);
}

/// Two bindings' `ending`s write at places of their own: neither takes
/// the other's key, and neither is answered from the other's entry.
#[tokio::test]
async fn two_bindings_endings_write_at_places_of_their_own() {
    let env = Env::new().await;
    let fake = FakeSurfaceProvider::new(env.store.clone());
    let mut run = fake_run(&env.run());
    // A second binding, of another surface, given to the same fake.
    run.bindings.push(Binding {
        limits: clax_bot::surface::SurfaceLimits::default(),
        surface: "other".into(),
        ..FakeSurfaceProvider::binding()
    });
    let driver = DriverId::mint();
    env.store.claim_run(&run.id, &driver, 60).unwrap().unwrap();
    let step = env
        .store
        .begin_step(&run.id, &driver, 1, StepKind::Stopped)
        .unwrap();
    let cancel = CancelToken::new();
    let cx = |binding: usize| {
        StepCx::new(
            &run,
            &run.bindings[binding],
            1,
            step.attempt,
            &[],
            Claim::new(&env.store, &driver, &run.id),
            &cancel,
        )
    };
    assert_eq!(cx(0).notice(0).unwrap().seq, NOTICE_SEQ);
    assert_eq!(cx(1).notice(0).unwrap().seq, NOTICE_SEQ + NOTICE_STRIDE);
    let first = fake
        .ending(cx(0), &End::stopped("disabled"), None)
        .await
        .unwrap();
    let second = fake
        .ending(cx(1), &End::stopped("no_flag"), None)
        .await
        .unwrap();
    // With one place for both, the second would have met the first's
    // key (`key_reused`), or been answered the first's notice.
    assert_eq!(json!(first), json!({"fake_notice": "stopped: disabled"}));
    assert_eq!(json!(second), json!({"fake_notice": "stopped: no_flag"}));
    let keys: Vec<String> = fake.lock().applied.iter().map(|a| a.key.clone()).collect();
    assert_eq!(
        keys,
        [
            format!("{}:1:{NOTICE_SEQ}", run.id),
            format!("{}:1:{}", run.id, NOTICE_SEQ + NOTICE_STRIDE)
        ]
    );
}

#[tokio::test]
async fn the_fake_tells_a_run_loop_that_ends_a_run_out_of_order() {
    let f = Fixture::new().await;
    let end = End::GaveUp {
        code: "model".into(),
        reason: "No.".into(),
    };
    // `ended` before the terminal commit.
    f.fake.working(&f.run).await;
    let fields = f
        .fake
        .ending(f.cx(StepKind::GiveUp), &end, None)
        .await
        .unwrap();
    assert_eq!(json!(fields), json!({"fake_notice": "gave up: No."}));
    f.fake.ended(&f.run, &end, None).await.unwrap();
    let hooks: Vec<Hook> = f.fake.lock().hooks.iter().map(|h| h.hook).collect();
    assert_eq!(hooks, [Hook::Working, Hook::Ending, Hook::Ended]);
    let wrong = f.fake.lifecycle(&f.run.id).unwrap_err();
    assert!(wrong.contains("before its terminal commit"), "{wrong}");
    // Nothing at all.
    let other = fake_run(&f.env.run());
    let wrong = f.fake.lifecycle(&other.id).unwrap_err();
    assert!(wrong.contains("called 0 times"), "{wrong}");
}

/// `ended` may be told more than once after the terminal commit, with
/// the same end; another end, or another hook after it, is the run
/// loop's mistake.
#[tokio::test]
async fn the_fake_takes_ended_again_and_tells_what_else_follows_it() {
    let f = Fixture::new().await;
    let end = End::stopped("disabled");
    let fields = f
        .fake
        .ending(f.cx(StepKind::Stopped), &end, None)
        .await
        .unwrap();
    f.env
        .store
        .commit_bot_step(
            &clax_bot::run::bot_ctx(&f.run.person),
            &f.run.id,
            &f.driver,
            1,
            clax_store::bot_runs::StepCommit {
                payload: json!(fields),
                reason: Some("disabled".into()),
                ..Default::default()
            },
        )
        .unwrap();
    for _ in 0..3 {
        f.fake.ended(&f.run, &end, None).await.unwrap();
        assert_eq!(f.fake.lifecycle(&f.run.id), Ok(()));
    }
    assert_eq!(
        f.fake.lock().followed,
        std::slice::from_ref(&f.run.id),
        "acted on once"
    );
    // Told the same kind of end without its details, as a repeat
    // rebuilt after a restart is.
    let (full, bare) = (
        End::Stopped {
            reason: "disabled".into(),
            limit: Some("40 steps".into()),
            credits_reset: None,
        },
        End::stopped("run_cap"),
    );
    for end in [&full, &bare] {
        f.fake.ended(&f.run, end, None).await.unwrap();
        assert_eq!(f.fake.lifecycle(&f.run.id), Ok(()));
    }
    // Told another kind of end.
    f.fake.ended(&f.run, &End::Finished, None).await.unwrap();
    let wrong = f.fake.lifecycle(&f.run.id).unwrap_err();
    assert!(wrong.contains("when it was called again"), "{wrong}");
    f.fake.lock().hooks.pop();
    // Another hook after it.
    f.fake.working(&f.run).await;
    let wrong = f.fake.lifecycle(&f.run.id).unwrap_err();
    assert!(
        wrong.contains("Working was called after `ended`"),
        "{wrong}"
    );
}

/// A case the fixture cannot play fails unless the caller names it, and
/// a case the caller names fails when it was played: neither the
/// fixture nor the test can leave a case out alone.
#[tokio::test]
async fn a_case_is_left_out_only_when_the_fixture_and_the_caller_both_say_so() {
    let env = Env::new().await;
    let failures =
        |results: Vec<(&'static str, Result<(), String>)>| -> Vec<(&'static str, String)> {
            results
                .into_iter()
                .filter_map(|(name, r)| r.err().map(|e| (name, e)))
                .collect()
        };
    // The caller names nothing: the case the fake cannot be played
    // through fails, with the fixture's reason.
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let failed = failures(check_provider(&fake_fixture(&env, &fake), &[]).await);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].0, STAGED);
    assert!(
        failed[0].1.contains("the fake stages nothing")
            && failed[0].1.contains("does not name it as not applicable"),
        "{}",
        failed[0].1
    );
    // The caller names a case that is played, and one that is none.
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let played = "a_begin_that_fails_leaves_the_step_to_be_begun_again";
    let named = [STAGED, played, "no_such_case"];
    let failed = failures(check_provider(&fake_fixture(&env, &fake), &named).await);
    let names: Vec<&str> = failed.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [
            played,
            "the_cases_named_as_not_applicable_are_cases_of_the_suite"
        ]
    );
    assert!(failed[0].1.contains("was played"), "{}", failed[0].1);
    assert!(failed[1].1.contains("no_such_case"), "{}", failed[1].1);

    // A reason that says nothing is no reason.
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let mut fixture = fake_fixture(&env, &fake);
    fixture.staged = Maybe::Lacks("  ");
    let failed = failures(check_provider(&fixture, &[STAGED]).await);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].0, "the_fixture_says_why_it_lacks_what_it_lacks");
    assert!(failed[0].1.contains("staged"), "{}", failed[0].1);
}

/// A read that nothing moves cannot tell an answer from the journal
/// from one made afresh: with such a read and no `disturb`, the case of
/// the replayed read is not played, and must be named by the caller.
#[tokio::test]
async fn a_read_that_nothing_moves_does_not_play_the_case_of_the_replayed_read() {
    let env = Env::new().await;
    let read_case = "a_reads_answer_is_the_providers_and_its_replay_the_journals";
    let unmoved = |fake: &Arc<FakeSurfaceProvider>| {
        let mut fixture = fake_fixture(&env, fake);
        // A value no case sets: the same answer each time.
        fixture.read = OpCall::new("fake.get", json!(["never set"]));
        fixture.disturb = Maybe::Lacks("this fixture's read is of a value nothing sets");
        fixture
    };
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let results = check_provider(&unmoved(&fake), &[STAGED]).await;
    let failed: Vec<_> = results.iter().filter(|(_, r)| r.is_err()).collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].0, read_case);
    let why = failed[0].1.as_ref().unwrap_err();
    assert!(why.contains("a value nothing sets"), "{why}");
    // Named by the caller, the suite passes.
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    for (name, result) in check_provider(&unmoved(&fake), &[STAGED, read_case]).await {
        result.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // A `disturb` that moves nothing is the fixture's mistake.
    let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
    let mut fixture = unmoved(&fake);
    fixture.disturb = Maybe::Has(Box::new(|| {}));
    let results = check_provider(&fixture, &[STAGED]).await;
    let failed: Vec<_> = results.iter().filter(|(_, r)| r.is_err()).collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0].1.as_ref().unwrap_err().contains("did not change"),
        "{failed:?}"
    );
}

/// A provider whose `undeclared` answers as if it had journaled the
/// call, or with a value.
struct Careless {
    fake: FakeSurfaceProvider,
    answer: Answer,
}

#[async_trait::async_trait]
impl SurfaceProvider for Careless {
    fn definition(&self) -> &clax_surface::Definition {
        self.fake.definition()
    }

    fn undeclared(&self, _op: &str, _args: &[Value]) -> Option<Answer> {
        Some(self.answer.clone())
    }

    async fn begin<'s>(
        &'s self,
        cx: StepCx<'s>,
    ) -> clax_core::Result<Box<dyn clax_bot::surface::StepSession + 's>> {
        self.fake.begin(cx).await
    }

    async fn ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<clax_bot::surface::Fields> {
        self.fake.ending(cx, end, result).await
    }

    async fn ended(
        &self,
        run: &RunInfo,
        end: &End,
        result: Option<&Value>,
    ) -> Result<(), HostError> {
        self.fake.ended(run, end, result).await
    }
}

/// Whatever `undeclared` answers, the core journals a refusal: the hook
/// has no place and no claim, so it journaled nothing itself.
#[tokio::test]
async fn what_undeclared_answers_is_journaled_by_the_core_as_a_refusal() {
    let own = HostError::new("forbidden", "not for this run");
    for (answer, refusal) in [
        (Answer::written(Err(own.clone())), own.clone()),
        (
            Answer::written(Ok(json!("performed"))),
            HostError::new(
                "unknown_op",
                "clax.fake.hidden is not an operation this run has",
            ),
        ),
    ] {
        let f = Fixture::new().await;
        let careless = Careless {
            fake: FakeSurfaceProvider::new(f.env.store.clone()),
            answer,
        };
        let cx = f.cx(StepKind::Script);
        let session = careless.begin(cx).await.unwrap();
        let bound = [Bound {
            binding: cx.binding,
            provider: &careless,
            session: &*session,
        }];
        let place = cx.place(0).unwrap();
        let raw = json!(["x"]);
        let answer = dispatch(&bound, &cx.claim, &place, "fake.hidden", &raw, None).await;
        assert_eq!(answer.journal(), Journal::Outcome);
        assert_eq!(answer.result(), &Err(refusal.clone()));
        let journal = f.env.store.bot_calls(&f.run.id, 1).unwrap();
        assert_eq!(journal.len(), 1, "the refusal has its row");
        assert_eq!(recorded(&journal[0]), Err(refusal));
        assert!(careless.fake.lock().calls.is_empty(), "no session saw it");
    }
}

/// A script's call past the places a step has for a script's calls is
/// refused before anything is routed, journaled or performed.
#[tokio::test]
async fn a_call_past_a_steps_places_has_no_place() {
    let f = Fixture::new().await;
    let cx = f.cx(StepKind::Script);
    assert!(cx.place(NOTICE_SEQ - 1).is_some());
    assert_eq!(cx.place(NOTICE_SEQ), None);
    // The first notice's place is a binding's, not a script's.
    assert_eq!(cx.notice(0).unwrap().seq, NOTICE_SEQ);
    let refusal = clax_bot::surface::no_place(NOTICE_SEQ);
    assert_eq!(refusal.code, "call_limit");
    // The sandbox's limit does not stand for this one: the setting
    // behind it takes a value above the first notice's place.
    f.env.admin_sets(&[(
        clax_store::bot_settings::Setting::ScriptMaxCalls,
        json!(NOTICE_SEQ + 1),
    )]);
}

// ---------------------------------------------------------------------------
// Clax's own provider
// ---------------------------------------------------------------------------

/// Clax's own provider, with two faults a test can arm. Its next
/// `begin` is given a history whose last step says the draft is based
/// on a version the artifact does not have, so that the provider's own
/// `begin` fails as it does for a draft that cannot be loaded. Its next
/// `ending` fails after the provider's own has answered, and so after
/// its notices are posted. Every other call is the provider's,
/// unchanged; a fake beside it is told each `ending` and `ended` too,
/// and keeps the record of them the cases read.
struct ClaxWithALostBase {
    clax: ClaxProvider,
    lose_the_base: AtomicBool,
    fail_the_ending: AtomicBool,
    told: FakeSurfaceProvider,
}

#[async_trait::async_trait]
impl SurfaceProvider for ClaxWithALostBase {
    fn definition(&self) -> &Definition {
        self.clax.definition()
    }

    fn standing(&self, op: &str) -> bool {
        self.clax.standing(op)
    }

    fn undeclared(&self, op: &str, args: &[Value]) -> Option<Answer> {
        self.clax.undeclared(op, args)
    }

    async fn working(&self, run: &RunInfo) {
        self.clax.working(run).await;
    }

    async fn begin<'s>(&'s self, cx: StepCx<'s>) -> clax_core::Result<Box<dyn StepSession + 's>> {
        if !self.lose_the_base.swap(false, Ordering::SeqCst) {
            return self.clax.begin(cx).await;
        }
        // A committed step that says the draft moved to a version the
        // artifact does not have.
        let mut history = cx.history.to_vec();
        history.push(Step {
            run_id: cx.run.id.clone(),
            n: 0,
            kind: StepKind::Script,
            started_at: "2026-10-21T10:00:00Z".into(),
            attempt: 1,
            committed_at: Some("2026-10-21T10:00:00Z".into()),
            payload: Some(json!({"base": 4_000_000})),
            observed: Vec::new(),
            failures: 0,
        });
        let lost = StepCx::new(
            cx.run, cx.binding, cx.step, cx.attempt, &history, cx.claim, cx.cancel,
        );
        match self.clax.begin(lost).await {
            Err(e) => Err(e),
            Ok(_) => panic!("a draft based on a version the artifact lacks was loaded"),
        }
    }

    async fn ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<Fields> {
        let _ = self.told.ending(cx, end, result).await;
        let fields = self.clax.ending(cx, end, result).await?;
        // A failure that comes after the notices are posted: the
        // attempt made again must not post them twice.
        if self.fail_the_ending.swap(false, Ordering::SeqCst) {
            return Err(clax_core::CoreError::invalid(
                "broken",
                "the ending is broken, once",
            ));
        }
        Ok(fields)
    }

    async fn ended(
        &self,
        run: &RunInfo,
        end: &End,
        result: Option<&Value>,
    ) -> Result<(), HostError> {
        let _ = self.told.ended(run, end, result).await;
        self.clax.ended(run, end, result).await
    }
}

/// The cases Clax's own provider is not played through, for the reasons
/// its fixture gives.
const NOT_CLAXS: [&str; 3] = [
    "an_unwritten_failure_leaves_no_row_and_is_made_again",
    "a_waiting_call_leaves_no_row_is_applied_once_and_is_answered_from_its_entry",
    "a_waiting_answer_from_an_operation_not_tagged_waits_is_the_providers_error",
];

/// Clax's own provider through the provider conformance suite: a
/// document read that the fixture changes from outside the run as the
/// suite's read, a host write (`threads.reply`) as its write, a reply on
/// a thread outside the request as the refusal, a web operation the run
/// did not declare as the operation outside the binding, a draft write
/// as the write that waits for its step's commit, a draft that cannot
/// be loaded as the `begin` that fails, and a run stopped because the
/// instance turned ClaxBot off as the run that ends: its notice is
/// posted on the run's thread once, however many times `ending` is
/// called, and `ended` told twice turns the roster off twice and
/// changes nothing a person sees.
#[tokio::test]
async fn claxs_own_provider_passes_the_provider_conformance_suite() {
    let env = Arc::new(Env::new().await);
    env.set_doc("notes/a", json!({"n": 0}));
    // The admin has the web search on and the host has a source that
    // answers it: only the run's binding keeps a search from the source.
    env.admin_sets(&[(Setting::WebSearchEnabled, json!(true))]);
    env.web().search_answers(
        "clax",
        Ok(SearchAnswer {
            hits: Vec::new(),
            request_id: None,
            raw: json!({}),
        }),
    );
    let provider = Arc::new(ClaxWithALostBase {
        clax: ClaxProvider::new(
            env.store.clone(),
            env.host.clone(),
            Some(env.web() as Arc<dyn WebSource>),
            env.clock.clone(),
        ),
        lose_the_base: AtomicBool::new(false),
        fail_the_ending: AtomicBool::new(false),
        told: FakeSurfaceProvider::new(env.store.clone()),
    });
    let info = |id: String| env.info(&id);
    let (run, cancelled_run, ending_run) = (info(env.run()), info(env.run()), info(env.run()));
    // ClaxBot's replies on the run's thread, in the store.
    let state = {
        let env = env.clone();
        move || Ok(json!(env.host.replies(&[&env.t1])))
    };
    let staged_draft = {
        let (env, run) = (env.clone(), cancelled_run.id.clone());
        move || {
            let files = env.store.draft_files(&run).map_err(|e| e.to_string())?;
            Ok(json!(files.iter().map(|f| &f.path).collect::<Vec<_>>()))
        }
    };
    // The notices the end of the run the cases end posted: the journal
    // rows at its terminal step's notice places, each with the attempt
    // that wrote it, and every reply ClaxBot left on the run's thread,
    // as the store holds them.
    let noticed = {
        let (env, run) = (env.clone(), ending_run.id.clone());
        move || {
            let rows = env.store.bot_calls(&run, 1).map_err(|e| e.to_string())?;
            let places: Vec<Value> = rows
                .iter()
                .filter(|c| c.seq() >= NOTICE_SEQ)
                .map(
                    |c| json!({"seq": c.seq(), "attempt": c.attempt(), "op": c.op(), "ok": c.ok()}),
                )
                .collect();
            Ok(json!({"places": places, "replies": env.host.replies(&[&env.t1])}))
        }
    };
    // The document the read is of, written again from outside the run.
    let disturb = {
        let (env, n) = (env.clone(), AtomicU64::new(1));
        move || env.set_doc("notes/a", json!({"n": n.fetch_add(1, Ordering::SeqCst)}))
    };
    let fail_once = {
        let provider = provider.clone();
        move |hook: Hook| match hook {
            Hook::Begin => {
                provider.lose_the_base.store(true, Ordering::SeqCst);
                Ok(())
            }
            Hook::Ending => {
                provider.fail_the_ending.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err("the fixture makes only `begin` and `ending` fail"),
        }
    };
    let lifecycle = {
        let provider = provider.clone();
        move |run: &str| provider.told.lifecycle(run)
    };
    let fixture = ProviderFixture {
        provider: provider.clone(),
        store: env.store.clone(),
        run,
        read: OpCall::new("db.get", json!(["notes", "a"])),
        disturb: Maybe::Has(Box::new(disturb)),
        write: OpCall::new("threads.reply", json!([env.t1, "First."])),
        second_write: OpCall::new("threads.reply", json!([env.t1, "Second."])),
        state: Box::new(state),
        // A thread of the artifact that is not the run's.
        refused: OpCall::new("threads.reply", json!([env.t2, "Not mine."])),
        // The run declared no web operation.
        outside: OpCall::new("web.search", json!(["clax", null])),
        cancelled_run,
        staged: Maybe::Has(Staged {
            write: OpCall::new("files.write", json!(["extra.css", "h1 {}"])),
            state: Box::new(staged_draft),
        }),
        ending_run,
        end: End::stopped("disabled"),
        result: None,
        noticed: Box::new(noticed),
        unwritten: Maybe::Lacks(
            "Clax's own provider leaves a call unwritten only when the store refuses the \
             transaction of a web call, which takes the ID of the driver that holds the run: \
             the cases mint their own (`web::a_web_call_whose_transaction_fails_is_made_again` \
             drives it through the run loop)",
        ),
        waiting: Maybe::Lacks("Clax's own surface has no operation tagged @waits"),
        untagged: Maybe::Lacks(
            "Clax's own provider has no answer that a call waits: none of its sessions' code \
             builds one",
        ),
        fail_once: Maybe::Has(Box::new(fail_once)),
        lifecycle: Maybe::Has(Box::new(lifecycle)),
    };
    let ending_run = fixture.ending_run.id.clone();
    let results = check_provider(&fixture, &NOT_CLAXS).await;
    assert_eq!(results.len(), 17);
    for (name, result) in results {
        result.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    assert!(
        !provider.lose_the_base.load(Ordering::SeqCst),
        "the `begin` that fails was played"
    );
    // The operation outside the binding reached no web source, through
    // the core or past it.
    assert_eq!(env.web().calls().len(), 0);
    // Three replies stand, each made once: two by the step the cases
    // committed, one by the step they cancelled.
    let replies: Vec<String> = env
        .host
        .replies(&[&env.t1])
        .into_iter()
        .map(|(_, body)| body)
        .collect();
    // And the notice of the run that was stopped, once, though its
    // `ending` was called for four attempts of its terminal step; the
    // cancelled run left none.
    assert_eq!(
        replies,
        [
            "First.",
            "Second.",
            "First.",
            "I stopped: ClaxBot is turned off on this Clax. I had not run any scripts yet."
        ]
    );
    let places: Vec<(i64, i64)> = env
        .store
        .bot_calls(&ending_run, 1)
        .unwrap()
        .iter()
        .map(|c| (c.seq(), c.attempt()))
        .collect();
    assert_eq!(
        places,
        [(NOTICE_SEQ, 1)],
        "the notice's one row, of the attempt that posted it"
    );
    let host = env.host.lock();
    assert!(
        !provider.fail_the_ending.load(Ordering::SeqCst),
        "the `ending` that fails was played"
    );
    // `ended` turned the roster off each time it was told, after the
    // run had ended: once for the cancelled run, twice for the other.
    let off: Vec<_> = host.roster.iter().filter(|c| !c.on).collect();
    assert_eq!(off.len(), 3, "{:?}", host.roster);
    assert!(
        off.iter().all(|c| c.status == Some(RunStatus::Stopped)),
        "{off:?}"
    );
}

/// A provider made for a run with the run's row opens that run's
/// session without reading the row from the store: the row here is of a
/// run the store does not hold. Made without it, the provider reads the
/// row, and finds none.
#[tokio::test]
async fn a_provider_given_its_runs_row_does_not_read_it_from_the_store() {
    let env = Env::new().await;
    let (mut row, made) = env.run_as_made(&env.run());
    row.id = "01NOSUCHRUN0000000000000000".into();
    let plain = ClaxProvider::new(env.store.clone(), env.host.clone(), None, env.clock.clone());
    let info = env.deps().providers.bind(&row, &made).unwrap().info;
    let driver = DriverId::mint();
    let cancel = CancelToken::new();
    let cx = StepCx::new(
        &info,
        &info.bindings[0],
        1,
        1,
        &[],
        Claim::new(&env.store, &driver, &row.id),
        &cancel,
    );
    assert!(
        plain.begin(cx).await.is_err(),
        "the store holds no such run"
    );
    let given = ClaxProvider::new(env.store.clone(), env.host.clone(), None, env.clock.clone())
        .for_run(row.clone());
    let session = given.begin(cx).await.expect("a session over the given row");
    // The session is of that row: a thread of its request is its own.
    let parts = session.finish(StepEnd::Commit);
    assert_eq!(parts.payload["replied"], json!([]));
    // A provider given another run's row reads this run's.
    let other = ClaxProvider::new(env.store.clone(), env.host.clone(), None, env.clock.clone())
        .for_run(env.run_row(&env.run()));
    assert!(other.begin(cx).await.is_err());
    // The provider a tick holds for a run is one given the row the tick
    // read (`SurfaceProvider::for_run`): it too opens the session of
    // the run the store does not hold.
    let bound = env.deps().providers.bind(&row, &made).unwrap();
    let held = bound.kind_provider().begin(cx).await;
    assert!(held.is_ok(), "the tick's provider read the row again");
}
