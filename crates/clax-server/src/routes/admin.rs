//! ClaxBot's admin pages (spec
//! `docs/superpowers/specs/2026-10-21-claxbot-design.md` §5, §7, §8, §10):
//! people and their flags, roles and lifts; the provider, its key, its
//! models and the run caps; pricing; and the week's usage.
//!
//! - `GET /api/admin/people` → `[person]`; `PUT
//!   /api/admin/people/<person>/flags {claxbot?, claxbot_unenforced?}`,
//!   `…/role {role}` (the owner alone: else 403 `owner_only`) and
//!   `…/credits {lift_milli, reason}` → the person. `<person>` is a person
//!   key, URL-encoded (`%2Aowner`).
//! - `GET/PUT /api/admin/provider`, `PUT/DELETE /api/admin/provider/key`,
//!   `POST /api/admin/provider/test`, `GET /api/admin/provider/models`.
//!   A field the daemon's configuration sets (`config.toml` or the
//!   environment) shows its source and is refused 409
//!   `setting_overridden` here. The instance key is write-only; a key in
//!   `[bot] api_key_env`'s variable takes precedence over a stored one
//!   (`sources.key` `env:<NAME>`).
//! - `GET/PUT /api/admin/pricing`: the credit price, the free tier and
//!   the cost lines; `PUT` takes the whole object (a line left out is
//!   deleted).
//! - `GET /api/admin/usage?week=`.
//!
//! Who may: the owner and admins ([`clax_http::bot::may_admin`]); agents
//! get 403 `not_for_agents`. Every page view is recorded as `view.admin
//! {page, subject_person?}` (at most once in five minutes per viewer and
//! page); every change by the store's own kinds (`admin.flag`,
//! `admin.role`, `admin.credits`, `admin.provider`, `admin.key`,
//! `admin.cost_line`).

use super::artifacts::{body, path};
use super::bot::{line_labels, line_total, week_at, week_of};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::CoreError;
use clax_core::audit::AuditKind;
use clax_http::agent::DeniedToGrants;
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::bot::{Bot, Names, may_admin, owner_only, record_view};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_sample::messages::{ApiKey, MessagesClient};
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::credits::{Applies, CostLinePatch, Unit};
use clax_store::roles::Role;
use clax_store::secrets::SecretSlot;
use serde::Deserialize;
use serde_json::{Value, json};

/// The caller, refused unless an admin (agents 403 `not_for_agents`).
async fn admin(s: &AppState, d: &DeniedToGrants) -> Result<(Caller, Role), ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(s).await?;
    let c = caller.clone();
    let role = s.store_call(move |st| Ok(may_admin(st, &c))).await??;
    Ok((caller, role))
}

/// Records `view.admin {page, subject_person?}`.
fn viewed(
    st: &Store,
    bot: &Bot,
    who: &Identity,
    caller: &Caller,
    page: &str,
    subject: Option<&str>,
) -> clax_core::Result<()> {
    let mut rec = clax_http::bot::view(st, AuditKind::ViewAdmin).with("page", page);
    if let Some(p) = subject {
        rec = rec.with("subject_person", p);
    }
    record_view(
        st,
        bot,
        who,
        caller,
        &format!("{page}/{}", subject.unwrap_or("")),
        rec,
    )
}

// ---------------------------------------------------------------------------
// People
// ---------------------------------------------------------------------------

/// Every person as the admin pages list them, this week.
fn people_view(st: &Store, bot: &Bot) -> clax_core::Result<Vec<Value>> {
    let at = st.now_utc();
    let runs = st.runs_in_week(at)?;
    let refused = st.refused_in_week(at)?;
    st.people_for_admin(at)?
        .into_iter()
        .map(|p| {
            let key_source = super::bot::key_source(st, bot, &p.person)?
                .unwrap_or(p.key_source)
                .as_str();
            Ok(json!({
                "person": p.person, "name": p.name, "role": p.role.as_str(),
                "flags": p.flags, "key_source": key_source,
                "week": {"weekly_milli": p.week.weekly_milli, "lift_milli": p.week.lift_milli,
                         "spent_milli": p.week.spent_milli},
                "tokens_week": p.tokens_week,
                "runs_week": runs.get(&p.person).copied().unwrap_or(0),
                "refused_week": refused.get(&p.person).copied().unwrap_or(0),
            }))
        })
        .collect()
}

/// Person `person` as [`people_view`] lists them, or `NotFound`.
fn person_view(st: &Store, bot: &Bot, person: &str) -> clax_core::Result<Value> {
    people_view(st, bot)?
        .into_iter()
        .find(|p| p["person"] == person)
        .ok_or(CoreError::NotFound)
}

/// `GET /api/admin/people` (see the module documentation).
pub async fn people(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = people_view(st, &bot)?;
            viewed(st, &bot, &who, &caller, "people", None)?;
            Ok(Value::Array(v))
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagsBody {
    #[serde(default)]
    claxbot: Option<bool>,
    #[serde(default)]
    claxbot_unenforced: Option<bool>,
}

/// `PUT /api/admin/people/<person>/flags` (see the module documentation).
pub async fn set_flags(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<FlagsBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let person = path(p)?;
    let b = body(req)?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            use clax_store::flags::Flag;
            let changes: Vec<(Flag, bool)> = [
                (Flag::Claxbot, b.claxbot),
                (Flag::ClaxbotUnenforced, b.claxbot_unenforced),
            ]
            .into_iter()
            .filter_map(|(f, v)| v.map(|v| (f, v)))
            .collect();
            let ctx = audit.resolve(st)?;
            st.set_flags(&ctx, &person, &changes)?;
            person_view(st, &bot, &person)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBody {
    role: String,
}

/// `PUT /api/admin/people/<person>/role {role: "admin"|"member"}`: the
/// owner alone (403 `owner_only`); the owner's own role is refused 400
/// `owner_role`.
pub async fn set_role(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<RoleBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (_, role) = admin(&s, &d).await?;
    owner_only(role)?;
    let person = path(p)?;
    let b = body(req)?;
    let make_admin = match b.role.as_str() {
        "admin" => true,
        "member" => false,
        _ => {
            return Err(ApiError::bad_request(
                "invalid_role",
                "a role is admin or member",
            ));
        }
    };
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_admin(&ctx, &person, make_admin)?;
            person_view(st, &bot, &person)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditsBody {
    lift_milli: i64,
    #[serde(default)]
    reason: Option<String>,
}

/// `PUT /api/admin/people/<person>/credits {lift_milli, reason}` (see the
/// module documentation).
pub async fn set_credits(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<CreditsBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let person = path(p)?;
    let b = body(req)?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_lift(&ctx, &person, b.lift_milli, b.reason.as_deref())?;
            person_view(st, &bot, &person)
        })
        .await?;
    Ok(Json(v))
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// The run caps the provider page shows, by name, with their settings.
const CAPS: &[(&str, Setting)] = &[
    ("max_steps", Setting::MaxSteps),
    ("max_run_credits", Setting::MaxRunCredits),
    ("max_run_minutes", Setting::MaxRunMinutes),
    ("max_tokens", Setting::MaxTokens),
    ("concurrency", Setting::Concurrency),
];
/// The models, by tier, with their settings.
const MODELS: &[(&str, Setting)] = &[
    ("default", Setting::ModelDefault),
    ("quick", Setting::ModelQuick),
    ("complex", Setting::ModelComplex),
];

/// The provider as `GET /api/admin/provider` shows it.
///
/// # Errors
/// The store's.
pub fn provider_view(st: &Store, bot: &Bot) -> clax_core::Result<Value> {
    let eff = st.effective_bot_settings()?;
    let get = |s: Setting| eff.get(&s).map(|e| e.value.clone()).unwrap_or(Value::Null);
    let src = |s: Setting| {
        eff.get(&s)
            .map(|e| e.source.label())
            .unwrap_or_else(|| "default".into())
    };
    let mut names = Names::new(st);
    let (key, key_source) = match bot.env_key_for(st)? {
        Some(k) => (
            json!({"set": true, "last4": k.last4(), "set_at": null, "set_by": null,
                   "last_ok_at": null}),
            format!("env:{}", k.var),
        ),
        None => {
            let meta = st.secret_meta(&SecretSlot::Instance)?;
            let source = if meta.is_some() { "db" } else { "default" };
            let base_url = get(Setting::BaseUrl);
            (
                clax_http::bot::key_info(
                    &mut names,
                    meta.as_ref(),
                    base_url.as_str().unwrap_or_default(),
                    "the instance key",
                )?,
                source.to_string(),
            )
        }
    };
    let mut sources = serde_json::Map::new();
    sources.insert("base_url".into(), json!(src(Setting::BaseUrl)));
    sources.insert("key".into(), json!(key_source));
    let mut models = serde_json::Map::new();
    for (tier, s) in MODELS {
        models.insert((*tier).into(), get(*s));
        sources.insert(format!("models.{tier}"), json!(src(*s)));
    }
    let mut caps = serde_json::Map::new();
    for (name, s) in CAPS {
        caps.insert((*name).into(), get(*s));
        sources.insert(format!("caps.{name}"), json!(src(*s)));
    }
    Ok(json!({
        "provider": "anthropic",
        "base_url": get(Setting::BaseUrl),
        "models": models, "key": key, "caps": caps, "sources": sources,
    }))
}

/// `GET /api/admin/provider` (see the module documentation).
pub async fn provider(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = provider_view(st, &bot)?;
            viewed(st, &bot, &who, &caller, "provider", None)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBody {
    #[serde(default)]
    base_url: Option<Value>,
    #[serde(default)]
    models: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    caps: Option<serde_json::Map<String, Value>>,
}

/// `PUT /api/admin/provider {base_url?, models?, caps?}`: each field given
/// is set (null: back to its default). 400 `unknown_setting` for a model
/// tier or cap the page does not show; 409 `setting_overridden` for one
/// the daemon's configuration sets.
pub async fn set_provider(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<ProviderBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let mut changes: Vec<(Setting, Option<Value>)> = Vec::new();
    let some = |v: Value| (!v.is_null()).then_some(v);
    if let Some(u) = b.base_url {
        changes.push((Setting::BaseUrl, some(u)));
    }
    for (map, table, prefix) in [(b.models, MODELS, "models"), (b.caps, CAPS, "caps")] {
        for (k, v) in map.unwrap_or_default() {
            let s = table
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, s)| *s)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "unknown_setting",
                        format!("no setting is named {prefix}.{k}"),
                    )
                })?;
            changes.push((s, some(v)));
        }
    }
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_bot_settings(&ctx, &changes)?;
            provider_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/admin/provider/key {key}`: seals the instance key.
pub async fn set_key(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<super::bot::KeyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let bot = s.bot.clone();
    bot.kek()?;
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            st.set_secret(&ctx, kek, &SecretSlot::Instance, b.key())?;
            provider_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/admin/provider/key`: clears the stored instance key.
pub async fn clear_key(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.clear_secret(&ctx, &SecretSlot::Instance)?;
            provider_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// A key to send and the slot to mark when a request with it succeeds
/// (`None` for the environment's).
type KeyInUse = (ApiKey, Option<SecretSlot>);

/// The instance key in effect (the environment's while the base URL is
/// the owner's, `Bot::env_key_for`; else the stored one, with its slot to
/// mark when a request with it succeeds) and the base URL, for the
/// provider's own requests.
fn effective_key(st: &Store, bot: &Bot) -> clax_core::Result<(Option<KeyInUse>, String)> {
    let base_url = st
        .bot_setting(Setting::BaseUrl)?
        .as_str()
        .unwrap_or_default()
        .to_string();
    // A URL stored before the hosted policy applied is refused at use too.
    if !st.base_url_policy().allows(&base_url) {
        return Err(clax_core::CoreError::invalid(
            "invalid_setting",
            "base_url is not a URL a model key may be sent to here; set it again",
        ));
    }
    if let Some(k) = bot.env_key_for(st)? {
        return Ok((Some((ApiKey::new(k.key.expose()), None)), base_url));
    }
    let Some(kek) = &bot.kek else {
        return Ok((None, base_url));
    };
    let key = st
        .open_for_use(kek, &SecretSlot::Instance, &base_url)?
        .map(|s| (ApiKey::new(s.expose()), Some(SecretSlot::Instance)));
    Ok((key, base_url))
}

/// What [`models_of`] answers without an instance key.
const NO_KEY: &str = "no instance key is set";

/// The Messages client for `store`'s base URL policy: under a hosted
/// policy it connects only to public addresses
/// ([`clax_sample::messages::MessagesClient::public_only`]), so a public
/// name that resolves to a private one never receives the instance's key.
pub fn messages_client(store: &clax_store::Store) -> MessagesClient {
    match store.base_url_policy() {
        clax_store::bot_settings::BaseUrlPolicy::Hosted => MessagesClient::new().public_only(),
        _ => MessagesClient::new(),
    }
}

/// The provider's models with the instance key, or why not.
async fn models_of(
    s: &AppState,
) -> Result<
    (
        Result<Vec<clax_sample::messages::ModelInfo>, String>,
        Option<SecretSlot>,
    ),
    ApiError,
> {
    let bot = s.bot.clone();
    let (key, base_url) = s.store_call(move |st| effective_key(st, &bot)).await?;
    let Some((key, slot)) = key else {
        return Ok((Err(NO_KEY.into()), None));
    };
    let store = s.store.clone();
    // The provider's request is not `Send` on the Worker's one thread.
    let r = clax_rt::sendable(async move {
        messages_client(&store)
            .list_models(&base_url, &key)
            .await
            // Its own words, never the answer's text: the base URL may be
            // anyone's host.
            .map_err(|e| e.summary())
    })
    .await;
    Ok((r, slot))
}

/// `POST /api/admin/provider/test` → `{ok, latency_ms, error?}`: lists
/// the provider's models with the instance key and base URL in effect,
/// recorded as `view.admin {page: "provider.test"}`.
pub async fn test(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let started = web_time::Instant::now();
    let (r, slot) = models_of(&s).await?;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bot = s.bot.clone();
    let ok = r.is_ok();
    s.store_call(move |st| {
        if let (true, Some(slot)) = (ok, &slot) {
            st.mark_secret_ok(slot)?;
        }
        viewed(st, &bot, &who, &caller, "provider.test", None)
    })
    .await?;
    Ok(Json(match r {
        Ok(_) => json!({"ok": true, "latency_ms": latency_ms, "error": null}),
        Err(e) => json!({"ok": false, "latency_ms": latency_ms, "error": e}),
    }))
}

/// `GET /api/admin/provider/models` → `{models: [{id, display_name}],
/// fetched_at}`: the provider's `GET /v1/models` with the instance key
/// and base URL in effect (502 `provider_error` when it fails, 409
/// `no_key` without a key), recorded as `view.admin {page:
/// "provider.models"}`.
pub async fn models(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let (r, slot) = models_of(&s).await?;
    let bot = s.bot.clone();
    let ok = r.is_ok();
    let fetched_at = s
        .store_call(move |st| {
            if let (true, Some(slot)) = (ok, &slot) {
                st.mark_secret_ok(slot)?;
            }
            viewed(st, &bot, &who, &caller, "provider.models", None)?;
            Ok(st.now())
        })
        .await?;
    match r {
        Ok(list) => Ok(Json(json!({
            "models": list.iter().map(|m| json!({"id": m.id, "display_name": m.display_name}))
                .collect::<Vec<_>>(),
            "fetched_at": fetched_at,
        }))),
        Err(e) if e == NO_KEY => Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "no_key",
            "set the instance key to list the provider's models",
        )),
        Err(e) => Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "provider_error",
            e,
        )),
    }
}

// ---------------------------------------------------------------------------
// Pricing and usage
// ---------------------------------------------------------------------------

/// The pricing as `GET /api/admin/pricing` shows it.
fn pricing_view(st: &Store) -> clax_core::Result<Value> {
    let lines: Vec<Value> = st
        .cost_lines()?
        .into_iter()
        .map(|l| {
            json!({"id": l.id, "label": l.label, "unit": l.unit.as_str(),
                   "cost_micro_usd": l.cost_micro_usd, "margin_bp": l.margin_bp,
                   "fixed_credits_milli": l.fixed_credits_milli, "applies": l.applies.as_str(),
                   "source": l.source, "model": l.model})
        })
        .collect();
    let eff = st.effective_bot_settings()?;
    let src = |s: Setting| eff.get(&s).map(|e| e.source.label());
    Ok(json!({
        "credit_price_micro_usd": st.credit_price_micro_usd()?,
        "free_weekly_milli": st.free_weekly_milli()?,
        "lines": lines,
        "sources": {
            "credit_price_micro_usd": src(Setting::CreditPriceMicroUsd),
            "free_weekly_milli": src(Setting::FreeWeeklyMilli),
        },
    }))
}

/// `GET /api/admin/pricing` (see the module documentation).
pub async fn pricing(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = pricing_view(st)?;
            viewed(st, &bot, &who, &caller, "pricing", None)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineBody {
    id: String,
    label: String,
    unit: String,
    cost_micro_usd: i64,
    margin_bp: i64,
    #[serde(default)]
    fixed_credits_milli: Option<i64>,
    applies: String,
    source: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingBody {
    credit_price_micro_usd: i64,
    free_weekly_milli: i64,
    lines: Vec<LineBody>,
    /// What `GET` shows beside the rest; ignored.
    #[serde(default)]
    #[allow(dead_code)]
    sources: Option<Value>,
}

/// `PUT /api/admin/pricing` (see the module documentation): the credit
/// price and free tier (unless the daemon's configuration sets them, 409
/// `setting_overridden` when they differ), each line made or changed, and
/// the lines left out deleted (409 `cost_line_in_use` for one a run that
/// has not ended may charge), every value checked first and all of it in
/// one transaction ([`Store::replace_pricing`]): a refusal writes nothing.
pub async fn set_pricing(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<PricingBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let mut settings = Vec::new();
            for (s, v) in [
                (Setting::CreditPriceMicroUsd, b.credit_price_micro_usd),
                (Setting::FreeWeeklyMilli, b.free_weekly_milli),
            ] {
                if st.bot_setting(s)?.as_i64() != Some(v) {
                    settings.push((s, Some(json!(v))));
                }
            }
            let bad = |what: &str| CoreError::invalid("invalid_cost_line", what.to_string());
            // Every line checked first; then everything in one transaction,
            // so a refused line writes nothing.
            let mut lines = Vec::new();
            for l in b.lines {
                let patch = CostLinePatch {
                    label: Some(l.label),
                    unit: Some(Unit::parse(&l.unit).ok_or_else(|| bad("unknown unit"))?),
                    cost_micro_usd: Some(l.cost_micro_usd),
                    margin_bp: Some(l.margin_bp),
                    fixed_credits_milli: Some(l.fixed_credits_milli),
                    applies: Some(
                        Applies::parse(&l.applies)
                            .ok_or_else(|| bad("applies is instance_key or always"))?,
                    ),
                    source: Some(l.source),
                    model: Some(l.model.filter(|m| !m.trim().is_empty())),
                };
                lines.push((l.id, patch));
            }
            st.replace_pricing(&ctx, &settings, lines)?;
            pricing_view(st)
        })
        .await?;
    Ok(Json(v))
}

/// `GET /api/admin/usage?week=` (see the module documentation).
pub async fn usage(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    q: Result<Query<super::bot::WeekQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let q = q
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let at = week_at(st, q.week()).map_err(|e| CoreError::invalid(e.code, e.message))?;
            let r = st.usage_report(at)?;
            let labels = line_labels(st)?;
            let runs = st.runs_in_week(at)?;
            let refused = st.refused_in_week(at)?;
            let mut names = Names::new(st);
            let mut by_person = Vec::new();
            let (mut paid, mut free) = (0i64, 0i64);
            for p in &r.by_person {
                paid += p.total.credits_milli - p.total.free_milli;
                free += p.total.free_milli;
                by_person.push(json!({
                    "person": p.person, "name": names.name(&p.person)?,
                    "credits_milli": p.total.credits_milli, "free_milli": p.total.free_milli,
                    "cost_micro_usd": p.total.cost_micro_usd,
                    "runs": runs.get(&p.person).copied().unwrap_or(0), "tokens": p.tokens,
                }));
            }
            viewed(st, &bot, &who, &caller, "usage", None)?;
            Ok(json!({
                "week": week_of(at),
                "paid_milli": paid, "free_milli": free,
                "runs": runs.values().sum::<i64>(), "refused": refused.values().sum::<i64>(),
                "revenue_micro_usd": r.revenue_micro_usd,
                "cost_micro_usd": r.cost_micro_usd - r.free_tier_cost_micro_usd,
                "free_cost_micro_usd": r.free_tier_cost_micro_usd,
                "by_person": by_person,
                "by_line": r.by_line.iter().map(|(l, t)| line_total(&labels, l, t)).collect::<Vec<_>>(),
            }))
        })
        .await?;
    Ok(Json(v))
}
