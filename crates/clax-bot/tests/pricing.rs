//! What a turn is priced at: a model without cost lines at the catalog's
//! or the default rates, `require_explicit_price` refusing it on the
//! instance's key alone, `max_tokens` bounded by the credits left at those
//! rates, and a cost the provider billed charged as billed, across a
//! resumed run too.

use crate::common::*;
use clax_bot::run::CrashPoint;
use clax_bot::{Next, Refusal, Wait};
use clax_core::audit::AuditCtx;
use clax_sample::messages::ModelApi;
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::bot_settings::{Overrides, Setting};
use clax_store::credits::{Applies, CostLinePatch, KeySource, RateSource, UsageRow};
use clax_store::price_catalog::CatalogListing;
use clax_store::roles::OWNER;
use clax_store::secrets::SecretSlot;
use serde_json::json;

/// A model no cost line prices.
const UNPRICED: &str = "acme-model-1";

fn set(env: &Env, s: Setting, v: serde_json::Value) {
    env.store
        .set_bot_settings(&AuditCtx::DAEMON, &[(s, Some(v))])
        .unwrap();
}

fn own_key(env: &Env) {
    env.store
        .set_secret(
            &AuditCtx::DAEMON,
            &env.kek,
            &SecretSlot::Own(OWNER.into()),
            OWN_KEY,
        )
        .unwrap();
}

fn refusal(env: &Env, model: &str) -> Option<Refusal> {
    let settings = env.settings.0.lock().unwrap().clone();
    clax_bot::refusal(
        &env.store,
        &settings,
        &env.deps().keys,
        &*env.clock,
        OWNER,
        model,
    )
    .unwrap()
}

/// The usage rows of `model`'s token lines.
fn token_rows(env: &Env, model: &str) -> Vec<UsageRow> {
    let prefix = format!("{model}.");
    env.usage()
        .into_iter()
        .filter(|r| r.line.starts_with(&prefix))
        .collect()
}

fn finish_billed(billed: u64) -> Reply {
    stream_billed(
        &[B::Tool(
            "tu_fin",
            "finish",
            json!({"summary": "done"}).to_string(),
        )],
        "tool_use",
        USAGE,
        Some(billed),
    )
}

/// The run's model is listed by the catalog the base URL in effect names:
/// the store's base URL is Anthropic's (the turns still go to the fake
/// API, the run's settings' base URL).
fn catalog_lists(env: &Env, model: &str) {
    let mut ov = Overrides::default();
    ov.set(
        Setting::BaseUrl,
        &json!("https://api.anthropic.com"),
        "file",
    )
    .unwrap();
    env.store.set_bot_overrides(ov);
    env.store
        .replace_price_catalog(
            &[CatalogListing {
                provider_id: "anthropic".into(),
                model_id: model.into(),
                name: "Acme 1".into(),
                input_micro_usd: 3_000_000,
                output_micro_usd: 15_000_000,
                cache_read_micro_usd: Some(300_000),
                cache_write_micro_usd: None,
                source: "https://tokenulator.dev/data/site.json".into(),
                observed_at: "2026-10-20T00:00:00Z".into(),
            }],
            env.clock_now(),
        )
        .unwrap();
}

#[tokio::test]
async fn a_model_without_cost_lines_runs_at_the_default_rates() {
    let env = Env::new().await;
    assert_eq!(refusal(&env, UNPRICED), None);
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let got: Vec<(String, i64, i64, RateSource)> = token_rows(&env, UNPRICED)
        .into_iter()
        .map(|r| {
            let part = r.line.rsplit('.').next().unwrap().to_string();
            (part, r.qty, r.credits_milli, r.rate_source)
        })
        .collect();
    // 560 000, 28 000, 700 000 and 2 800 000 milli-credits a million tokens.
    assert_eq!(
        got,
        [
            ("input".to_string(), 1000, 560, RateSource::Default),
            ("cached_input".to_string(), 2000, 56, RateSource::Default),
            ("cache_write".to_string(), 500, 350, RateSource::Default),
            ("output".to_string(), 300, 840, RateSource::Default),
        ]
    );
}

#[tokio::test]
async fn a_model_the_catalog_lists_runs_at_its_rates() {
    let env = Env::new().await;
    catalog_lists(&env, UNPRICED);
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let rows = token_rows(&env, UNPRICED);
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|r| r.rate_source == RateSource::Catalog && r.credits_milli > 0),
        "{rows:?}"
    );
    let output = rows.iter().find(|r| r.line.ends_with(".output")).unwrap();
    assert_eq!(output.rate_cost_micro_usd, 15_000_000);
    assert_eq!(output.cost_micro_usd, 4_500);
}

#[tokio::test]
async fn require_explicit_price_refuses_an_unpriced_model_on_the_instance_key_alone() {
    let env = Env::new().await;
    let run = env.run_on(UNPRICED, KeySource::Instance);
    set(&env, Setting::RequireExplicitPrice, json!(true));
    assert_eq!(refusal(&env, UNPRICED), Some(Refusal::ModelNotPriced));
    assert_eq!(refusal(&env, "claude-sonnet-5-5"), None, "it has lines");
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(
        env.run_row(&run).reason.as_deref(),
        Some("model_not_priced")
    );
    assert!(env.api.requests().is_empty());
    // On the person's own key tokens cost them nothing here: no refusal.
    own_key(&env);
    assert_eq!(refusal(&env, UNPRICED), None);
}

#[tokio::test]
async fn max_tokens_is_bounded_by_the_credits_left_at_the_default_rates() {
    let env = Env::new().await;
    // 30 credits buy at most 10 714 output tokens at 2 800 000
    // milli-credits a million.
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    env.tick(&env.deps(), &run, &DriverId::mint()).await;
    let asked = env.api.requests()[0].body["max_tokens"].as_u64().unwrap();
    assert!((1024..=10_714).contains(&asked), "{asked}");
}

#[tokio::test]
async fn max_tokens_is_bounded_by_the_credits_left_at_the_catalogs_rates() {
    let env = Env::new().await;
    catalog_lists(&env, UNPRICED);
    // 30 credits buy at most 14 285 output tokens at $15 a million, 40 %
    // margin and a $0.01 credit (2 100 000 milli-credits a million).
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    env.tick(&env.deps(), &run, &DriverId::mint()).await;
    let asked = env.api.requests()[0].body["max_tokens"].as_u64().unwrap();
    assert!((10_715..=14_285).contains(&asked), "{asked}");
}

#[tokio::test]
async fn an_own_key_is_not_bounded_by_token_rates_it_is_not_charged() {
    let env = Env::new().await;
    own_key(&env);
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Own);
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(env.api.requests()[0].body["max_tokens"], 32000);
    let rows = token_rows(&env, UNPRICED);
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter().all(|r| r.key_source == KeySource::Own
            && r.cost_micro_usd == 0
            && r.credits_milli == 0)
    );
}

#[tokio::test]
async fn a_billed_turn_is_charged_the_billed_cost() {
    let env = Env::new().await;
    env.api.push(finish_billed(12_345));
    let run = env.run();
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let rows = token_rows(&env, "claude-sonnet-5-5");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.iter().map(|r| r.cost_micro_usd).sum::<i64>(), 12_345);
    assert!(
        rows.iter().all(|r| r.rate_source == RateSource::Provider
            && r.credits_milli > 0
            && r.margin_bp == 4_000),
        "{rows:?}"
    );
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(
        steps[0].payload.as_ref().unwrap()["usage"]["billed_micro_usd"],
        12_345
    );
}

#[tokio::test]
async fn a_billed_turn_at_the_default_rates_keeps_their_credits() {
    let env = Env::new().await;
    env.api.push(finish_billed(9_000));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    let rows = token_rows(&env, UNPRICED);
    assert_eq!(rows.iter().map(|r| r.cost_micro_usd).sum::<i64>(), 9_000);
    let credits: Vec<i64> = rows.iter().map(|r| r.credits_milli).collect();
    assert_eq!(credits, [560, 56, 350, 840], "the fixed credits");
    assert!(
        rows.iter().all(|r| r.rate_source == RateSource::Default),
        "credit-based rows keep the source of their credits"
    );
}

#[tokio::test]
async fn a_billed_cost_survives_a_resumed_run() {
    let env = Env::new().await;
    env.api.push(finish_billed(12_345));
    env.api.push(finish_billed(23_456));
    let run = env.run();
    let mut a = env.deps();
    crate::resume::crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 1,
            kind: StepKind::Turn,
        },
    );
    assert_eq!(
        env.tick(&a, &run, &DriverId::mint()).await.next,
        Next::Waiting(Wait::Crashed)
    );
    env.clock.advance(61);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let rows = token_rows(&env, "claude-sonnet-5-5");
    let sum = |absorbed: bool| -> i64 {
        rows.iter()
            .filter(|r| r.absorbed == absorbed)
            .map(|r| r.cost_micro_usd)
            .sum()
    };
    assert_eq!(sum(true), 12_345, "the crashed call, absorbed as billed");
    assert_eq!(sum(false), 23_456, "the committed call, charged as billed");
    assert!(rows.iter().all(|r| r.rate_source == RateSource::Provider));
}

#[tokio::test]
async fn an_own_key_runs_an_unpriced_model_under_require_explicit_price() {
    let env = Env::new().await;
    own_key(&env);
    let run = env.run_on(UNPRICED, KeySource::Own);
    set(&env, Setting::RequireExplicitPrice, json!(true));
    env.api.push(finish_turn("done"));
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let rows = token_rows(&env, UNPRICED);
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter().all(|r| r.key_source == KeySource::Own
            && r.cost_micro_usd == 0
            && r.credits_milli == 0)
    );
}

#[tokio::test]
async fn the_input_bound_is_priced_at_the_cache_write_rate() {
    // The request's cache breakpoint has the provider bill a first turn's
    // whole prompt as cache writes: a dear cache-write rate leaves 30
    // credits short of the input, so no call is made.
    let env = Env::new().await;
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    set(&env, Setting::DefaultRateCacheWrite, json!(500_000_000));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(
        env.run_row(&run).reason.as_deref(),
        Some("credits_exhausted")
    );
    assert!(env.api.requests().is_empty());
    // The same price on uncached input does not bound the first turn.
    let env = Env::new().await;
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    set(&env, Setting::DefaultRateInput, json!(500_000_000));
    env.api.push(finish_turn("done"));
    let run = env.run_on(UNPRICED, KeySource::Instance);
    env.tick(&env.deps(), &run, &DriverId::mint()).await;
    let asked = env.api.requests()[0].body["max_tokens"].as_u64().unwrap();
    assert!((1024..=10_714).contains(&asked), "{asked}");
}

#[tokio::test]
async fn an_own_key_is_bounded_by_a_token_line_that_always_applies() {
    let env = Env::new().await;
    own_key(&env);
    env.store
        .put_cost_line(
            &AuditCtx::DAEMON,
            "claude-sonnet-5-5.output",
            CostLinePatch {
                applies: Some(Applies::Always),
                ..CostLinePatch::default()
            },
        )
        .unwrap();
    // 30 credits buy at most 21 428 output tokens at $10 a million, 40 %
    // margin and a $0.01 credit; the input lines are waived.
    set(&env, Setting::FreeWeeklyMilli, json!(30_000));
    env.api.push(finish_turn("done"));
    let run = env.run_on("claude-sonnet-5-5", KeySource::Own);
    env.tick(&env.deps(), &run, &DriverId::mint()).await;
    let asked = env.api.requests()[0].body["max_tokens"].as_u64().unwrap();
    assert_eq!(asked, 21_428);
    assert_eq!(env.api.requests()[0].header("x-api-key"), Some(OWN_KEY));
}

#[tokio::test]
async fn off_the_messages_api_the_input_bound_is_priced_at_the_input_rate() {
    // Without the Messages API's cache breakpoint, the provider bills the
    // uncached input as input: a dear cache-write rate does not bound the
    // turn, a dear input rate does.
    for api in [ModelApi::OpenAi, ModelApi::Gemini] {
        let env = Env::new().await;
        env.set(|s| s.api = api);
        set(&env, Setting::FreeWeeklyMilli, json!(30_000));
        set(&env, Setting::DefaultRateCacheWrite, json!(500_000_000));
        let run = env.run_on(UNPRICED, KeySource::Instance);
        let _ = env
            .try_tick(&env.deps(), &run, &DriverId::mint(), None)
            .await;
        let body = &env.api.requests()[0].body;
        let asked = match api {
            ModelApi::Gemini => &body["generationConfig"]["maxOutputTokens"],
            _ => &body["max_tokens"],
        }
        .as_u64()
        .unwrap();
        assert!((1024..=10_714).contains(&asked), "{api:?}: {asked}");

        let env = Env::new().await;
        env.set(|s| s.api = api);
        set(&env, Setting::FreeWeeklyMilli, json!(30_000));
        set(&env, Setting::DefaultRateInput, json!(500_000_000));
        let run = env.run_on(UNPRICED, KeySource::Instance);
        let out = env.drive(&env.deps(), &run, &DriverId::mint()).await;
        assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
        assert_eq!(
            env.run_row(&run).reason.as_deref(),
            Some("credits_exhausted"),
            "{api:?}"
        );
        assert!(env.api.requests().is_empty(), "{api:?}");
    }
}
