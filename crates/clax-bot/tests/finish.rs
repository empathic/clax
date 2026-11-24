//! `finish` carries the run's kind's result (`clax_bot::finish`): runs
//! with the fake surface alone (`clax_bot::surface::FakeSurfaceProvider`),
//! of the fake's kind `verdict` (`src/fake.d.ts`), whose model finishes
//! with a valid result, an invalid one and then a valid one, and invalid
//! ones up to the bound; and runs of Clax's own kind, whose `finish` is
//! read as it always was. The tool's schema for `verdict` is pinned in
//! `tests/golden/` (`golden::finish`).

use crate::common::*;
use clax_bot::Next;
use clax_bot::finish::INVALID_RESULT;
use clax_bot::policy::DEFAULT_INVALID_RESULTS;
use clax_bot::registry::Providers;
use clax_bot::run::Deps;
use clax_bot::surface::{End, FakeSurfaceProvider, Hook, SurfaceProvider};
use clax_store::bot_runs::{DriverId, RunStatus, Step, StepKind};
use serde_json::{Value, json};
use std::sync::Arc;

/// The fake's kind with a structured result.
pub const VERDICT: &str = "fake.verdict";

/// A home whose runs have the fake's binding alone.
pub struct Typed {
    pub env: Env,
    pub fake: Arc<FakeSurfaceProvider>,
}

impl Typed {
    pub async fn new() -> Typed {
        let env = Env::new().await;
        let fake = Arc::new(FakeSurfaceProvider::new(env.store.clone()));
        Typed { env, fake }
    }

    /// Deps for a run of `kind`: a registry with the fake alone for a
    /// kind of the fake's, Clax's own for `None`.
    pub fn deps(&self, kind: Option<&str>) -> Deps {
        match kind {
            Some(_) => {
                let provider: Arc<dyn SurfaceProvider> = self.fake.clone();
                self.env
                    .deps()
                    .with_providers(Providers::new().with(FakeSurfaceProvider::SURFACE, provider))
            }
            None => self.env.deps(),
        }
    }

    /// A new run of `kind`, a row whose one binding is the fake's
    /// surface; a run of Clax's `thread` for `None`.
    pub fn run(&self, kind: Option<&str>) -> String {
        let run = self.env.run();
        if let Some(kind) = kind {
            let bindings = json!({"v": 1, "bindings": [
                {"surface": FakeSurfaceProvider::SURFACE, "hash": null, "scope": null, "granted": []}]});
            crate::generic::set_row(
                &self.env,
                &run,
                "kind = ?2, artifact_id = NULL, base_version = NULL, bindings_json = ?3, \
                 request_json = '{\"ask\": \"Judge the case.\"}'",
                kind,
                &bindings.to_string(),
            );
        }
        run
    }

    /// Drives a new run of `kind` whose model's turns are `inputs`, each
    /// a call of `finish` with that input, to its end.
    pub async fn finishes(&self, kind: Option<&str>, inputs: &[Value]) -> String {
        for (i, input) in inputs.iter().enumerate() {
            self.env.api.push(finish_with(i, input));
        }
        self.drives(kind, &self.deps(kind)).await
    }

    /// Drives a new run of `kind` with `deps` to its end, its model's
    /// turns those pushed already.
    pub async fn drives(&self, kind: Option<&str>, deps: &Deps) -> String {
        let run = self.run(kind);
        let out = self.env.drive(deps, &run, &DriverId::mint()).await;
        assert!(matches!(out.last().unwrap().next, Next::Done(_)), "{out:?}");
        assert_eq!(self.env.api.left(), 0, "every turn was asked for");
        run
    }

    fn steps(&self, run: &str) -> Vec<Step> {
        self.env.store.run_with_steps(run).unwrap().unwrap().1
    }

    /// The results the fake's `hook` was given.
    fn told(&self, hook: Hook) -> Vec<(End, Option<Value>)> {
        self.fake
            .lock()
            .hooks
            .iter()
            .filter(|h| h.hook == hook)
            .map(|h| (h.end.clone().unwrap(), h.result.clone()))
            .collect()
    }

    /// What each turn after the first was told of the call before it:
    /// the tool result of the last user message of each request after
    /// the first.
    fn answered(&self) -> Vec<Value> {
        self.env.api.requests()[1..]
            .iter()
            .map(|r| {
                let messages = r.body["messages"].as_array().unwrap();
                let last = messages.last().unwrap()["content"].as_array().unwrap();
                let mut result = last.last().unwrap().clone();
                result.as_object_mut().unwrap().remove("cache_control");
                result
            })
            .collect()
    }
}

/// A turn that calls `finish` with `input`, the `i`th of its run.
pub fn finish_with(i: usize, input: &Value) -> Reply {
    const IDS: [&str; 8] = [
        "tu_1", "tu_2", "tu_3", "tu_4", "tu_5", "tu_6", "tu_7", "tu_8",
    ];
    stream(
        &[B::Tool(IDS[i], "finish", input.to_string())],
        "tool_use",
        USAGE,
    )
}

fn valid() -> Value {
    json!({"title": "Fair", "confidence": 7, "status": "upheld"})
}

/// A result `verdict`'s check refuses: a status it does not have.
fn invalid() -> Value {
    json!({"title": "Fair", "status": "maybe"})
}

const WHY: &str = "kind verdict: the result.status must be one of upheld, dismissed, deferred";

/// What a model is told of a refused result, as the call's tool result.
fn refused(id: &str) -> Value {
    json!({"type": "tool_result", "tool_use_id": id, "is_error": true,
           "content": format!("{INVALID_RESULT}: {WHY}")})
}

#[tokio::test]
async fn a_valid_result_ends_the_run_and_is_what_the_kinds_surface_is_told() {
    let t = Typed::new().await;
    let run = t.finishes(Some(VERDICT), &[valid()]).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(t.told(Hook::Ending), [(End::Finished, Some(valid()))]);
    assert_eq!(t.told(Hook::Ended), [(End::Finished, Some(valid()))]);
    // The terminal step keeps the result under the core's `result`,
    // and no `summary`.
    let steps = t.steps(&run);
    assert_eq!(
        steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
        [StepKind::Turn, StepKind::Finish]
    );
    assert_eq!(
        steps[0].payload.as_ref().unwrap()["next"],
        json!({"kind": "finish", "result": valid()})
    );
    assert_eq!(
        steps[1].payload.as_ref().unwrap(),
        &json!({"result": valid(),
                "fake": {"fake_notice": format!("finished: {}", valid())}})
    );
    assert_eq!(
        clax_bot::events::step_summary(StepKind::Finish, steps[1].payload.as_ref().unwrap()),
        json!({"summary": null, "result": valid()})
    );
}

#[tokio::test]
async fn an_invalid_result_is_answered_and_the_run_goes_on_to_a_valid_one() {
    let t = Typed::new().await;
    let run = t.finishes(Some(VERDICT), &[invalid(), valid()]).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(t.answered(), [refused("tu_1")]);
    let steps = t.steps(&run);
    assert_eq!(
        steps[0].payload.as_ref().unwrap()["next"],
        json!({"kind": "turn", "invalid_results": 1, "user": [refused("tu_1")]})
    );
    // The invalid result was told to no one.
    assert_eq!(t.told(Hook::Ended), [(End::Finished, Some(valid()))]);
    assert_eq!(
        steps.last().unwrap().payload.as_ref().unwrap()["result"],
        valid()
    );
}

#[tokio::test]
async fn invalid_results_up_to_the_bound_are_answered_and_the_next_one_gives_the_run_up() {
    let bound = usize::try_from(DEFAULT_INVALID_RESULTS).unwrap();
    // As many as the bound, then a valid one: the run finishes.
    let t = Typed::new().await;
    let mut inputs = vec![invalid(); bound];
    inputs.push(valid());
    let run = t.finishes(Some(VERDICT), &inputs).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    let ids = ["tu_1", "tu_2", "tu_3", "tu_4"];
    let told: Vec<Value> = ids[..bound].iter().map(|id| refused(id)).collect();
    assert_eq!(t.answered(), told);
    // One more: the run gives up, with the code that names the bound.
    let t = Typed::new().await;
    let run = t.finishes(Some(VERDICT), &vec![invalid(); bound + 1]).await;
    let row = t.env.run_row(&run);
    assert_eq!(
        (row.status, row.reason.as_deref()),
        (RunStatus::GaveUp, Some(INVALID_RESULT))
    );
    assert_eq!(t.answered(), told);
    let end = End::GaveUp {
        code: INVALID_RESULT.into(),
        reason: format!(
            "The model's result did not match the run's kind {} times in a row: {WHY}",
            bound + 1
        ),
    };
    assert_eq!(t.told(Hook::Ended), [(end, None)]);
}

#[tokio::test]
async fn a_step_that_is_not_a_refused_finish_ends_the_row() {
    let t = Typed::new().await;
    let bound = usize::try_from(DEFAULT_INVALID_RESULTS).unwrap();
    for i in 0..bound {
        t.env.api.push(finish_with(i, &invalid()));
    }
    // A turn that calls no tool is nudged: the count starts again.
    t.env.api.push(text_turn("Thinking it over."));
    for i in 0..bound {
        t.env.api.push(finish_with(i, &invalid()));
    }
    t.env.api.push(finish_with(bound, &valid()));
    let run = t.drives(Some(VERDICT), &t.deps(Some(VERDICT))).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
}

/// A run of Clax's own kind is read as it always was: a string
/// `summary` finishes it whatever else the input holds, which
/// `Kind::check_result` would refuse.
#[tokio::test]
async fn a_finish_of_claxs_kind_takes_a_summary_beside_keys_its_type_lacks() {
    let t = Typed::new().await;
    let run = t
        .finishes(None, &[json!({"summary": "done", "extra": 1})])
        .await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    let steps = t.steps(&run);
    assert_eq!(
        steps[0].payload.as_ref().unwrap()["next"],
        json!({"kind": "finish", "summary": "done"})
    );
    assert_eq!(steps[1].payload.as_ref().unwrap()["summary"], "done");
    assert!(steps[1].payload.as_ref().unwrap().get("result").is_none());
}

/// A summary that is not a string is answered that the input does not
/// match the schema, with nothing counted: past the bound a run of a
/// typed kind gives up at, the run still finishes.
#[tokio::test]
async fn a_finish_of_claxs_kind_without_a_string_summary_is_answered_without_a_bound() {
    let t = Typed::new().await;
    let bound = usize::try_from(DEFAULT_INVALID_RESULTS).unwrap();
    let mut inputs = vec![json!({"summary": 1}); bound + 2];
    inputs.push(json!({"summary": "done"}));
    let run = t.finishes(None, &inputs).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    let ids = ["tu_1", "tu_2", "tu_3", "tu_4", "tu_5", "tu_6"];
    let told: Vec<Value> = ids[..bound + 2]
        .iter()
        .map(|id| {
            json!({"type": "tool_result", "tool_use_id": id, "is_error": true,
                   "content": "The input of finish does not match its schema; nothing ran."})
        })
        .collect();
    assert_eq!(t.answered(), told);
    for step in &t.steps(&run)[..bound + 2] {
        assert!(
            step.payload.as_ref().unwrap()["next"]
                .get("invalid_results")
                .is_none()
        );
    }
}

/// A policy whose limits are the settings' with no bound on refused
/// results.
struct NoBound;

impl clax_bot::policy::RunPolicy for NoBound {
    fn limits(
        &self,
        _: &clax_bot::surface::RunInfo,
        settings: &clax_bot::RunSettings,
    ) -> clax_core::Result<clax_bot::policy::RunLimits> {
        let mut l = clax_bot::policy::RunLimits::of(settings);
        l.max_invalid_results = None;
        Ok(l)
    }
}

/// With no bound of the policy's, refused results are answered until
/// another of the run's bounds ends it: `None` is no bound, not a bound
/// of none.
#[tokio::test]
async fn with_no_bound_refused_results_go_on_until_another_cap_ends_the_run() {
    let t = Typed::new().await;
    let steps = 6;
    t.env.set(|s| s.max_steps = steps);
    for i in 0..usize::try_from(steps).unwrap() {
        t.env.api.push(finish_with(i, &invalid()));
    }
    let deps = t.deps(Some(VERDICT)).with_policy(Arc::new(NoBound));
    let run = t.drives(Some(VERDICT), &deps).await;
    let row = t.env.run_row(&run);
    assert_eq!(
        (row.status, row.reason.as_deref()),
        (RunStatus::Stopped, Some("run_cap"))
    );
    let ids = ["tu_1", "tu_2", "tu_3", "tu_4", "tu_5"];
    let told: Vec<Value> = ids.iter().map(|id| refused(id)).collect();
    assert_eq!(t.answered(), told);
}

/// A call of another tool whose input does not match its schema, between
/// refused results, ends the row as any other answer does.
#[tokio::test]
async fn another_tools_schema_error_between_refused_results_ends_the_row() {
    let t = Typed::new().await;
    let bound = usize::try_from(DEFAULT_INVALID_RESULTS).unwrap();
    for i in 0..bound {
        t.env.api.push(finish_with(i, &invalid()));
    }
    t.env.api.push(stream(
        &[B::Tool(
            "tu_s",
            "run_script",
            json!({"title": 1}).to_string(),
        )],
        "tool_use",
        USAGE,
    ));
    for i in 0..bound {
        t.env.api.push(finish_with(i, &invalid()));
    }
    t.env.api.push(finish_with(bound, &valid()));
    let run = t.drives(Some(VERDICT), &t.deps(Some(VERDICT))).await;
    assert_eq!(t.env.run_row(&run).status, RunStatus::Finished);
    let steps = t.steps(&run);
    let after = &steps[bound].payload.as_ref().unwrap()["next"];
    assert_eq!(
        after["user"][0]["content"],
        "The input of run_script does not match its schema; nothing ran."
    );
    assert!(after.get("invalid_results").is_none(), "{after}");
}

/// A run whose kind its surface's definition does not declare stops at
/// its first turn, before the model is asked: the request cannot be
/// rendered for a kind the definition lacks (`prompt::request_of`), so
/// the run stops `request_gone`, its error naming the kind. `finish`'s
/// own rule for such a kind (`invalid_result` up to the bound) is never
/// reached.
#[tokio::test]
async fn a_run_of_a_kind_its_surface_does_not_declare_stops_at_its_first_turn() {
    let t = Typed::new().await;
    let kind = Some("fake.nope");
    let run = t.run(kind);
    let out = t.env.drive(&t.deps(kind), &run, &DriverId::mint()).await;
    let row = t.env.run_row(&run);
    assert_eq!(
        (row.status, row.reason.as_deref()),
        (RunStatus::Stopped, Some("request_gone")),
        "{out:?}"
    );
    assert!(t.env.api.requests().is_empty());
    let steps = t.steps(&run);
    assert_eq!(steps.len(), 1);
    let error = steps[0].payload.as_ref().unwrap()["error"].to_string();
    assert!(error.contains("nope"), "{error}");
}
