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
//!   `api` names the wire API the base URL speaks (`anthropic`, `openai`,
//!   `gemini`).
//!   A field the daemon's configuration sets (`config.toml` or the
//!   environment) shows its source and is refused 409
//!   `setting_overridden` here. The instance key is write-only; a key in
//!   `[bot] api_key_env`'s variable takes precedence over a stored one
//!   (`sources.key` `env:<NAME>`). `kek {set, fingerprint}` says whether a
//!   key-encryption key seals the keys and which ([`Kek::fingerprint`]),
//!   for checking a backup of it; nothing public shows the fingerprint.
//!
//!   [`Kek::fingerprint`]: clax_store::secrets::Kek::fingerprint
//! - `GET/PUT /api/admin/web`: the web tools' settings, `{search: {enabled,
//!   provider, gateway, byok_alias, limit, max_per_run}, fetch: {enabled,
//!   url_rule, allowlist, max_per_run}, cloudflare: {account_id, token}}`,
//!   with `configured` (whether the host has a source at all) and `sources`
//!   keyed `search.provider`, `fetch.url_rule` and so on; `PUT` takes the
//!   same shape without `configured`, `sources` or `token`, any field may
//!   be left out and a null puts it back to its default. A value the
//!   setting's kind refuses is 400; a field the daemon's configuration
//!   sets is refused 409 `setting_overridden`, as the provider's are.
//!   `PUT/DELETE /api/admin/web/token` seals or clears the Cloudflare API
//!   token (503 `no_kek` without a key-encryption key); the token is
//!   write-only, shown as `key_info` (`last4`), and bound to Cloudflare's
//!   API origin whatever the model base URL is.
//! - `GET/PUT /api/admin/pricing`: the credit price, the free tier, the
//!   default token rates, `require_explicit_price` and the cost lines,
//!   with each priced model's rates and where they come from (`models`)
//!   and the price catalog's age (`catalog`); `PUT` takes the whole object
//!   (its `lines` are the lines that are not token lines, whose rates are
//!   under `models`; a line left out is deleted, a token line sent is
//!   refused and every one kept; `default_rates` and
//!   `require_explicit_price` may be left out). `POST
//!   /api/admin/pricing/models` sets a model's rate card and `DELETE
//!   /api/admin/pricing/models?model=` deletes it. `POST
//!   /api/admin/pricing/catalog/refresh` fetches the price catalog now;
//!   `POST /api/admin/pricing/tokenulator {model, provider?}` refreshes it
//!   and prices a model's token lines from it.
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
use clax_sample::messages::{ApiKey, MessagesClient, ModelApi};
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::credits::{
    Applies, CostLine, CostLinePatch, ModelPrices, ModelPricing, ModelRates, Rate, RateSource,
    TOKEN_MARGIN_BP, Unit, UnitPricing, UnitRate, credits_per_unit_milli, parse_token_line,
    token_line,
};
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
    sources.insert("api".into(), json!(src(Setting::Api)));
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
    // Which key-encryption key seals the keys: its fingerprint, so an
    // admin can check a backup of it (docs/prod.md, "CLAX_KEK"); `null`
    // without one. Never in public: /healthz says only whether there is one.
    let kek = bot.kek.as_ref().map(clax_store::secrets::Kek::fingerprint);
    Ok(json!({
        "api": get(Setting::Api),
        "base_url": get(Setting::BaseUrl),
        "models": models, "key": key, "caps": caps, "sources": sources,
        "kek": {"set": kek.is_some(), "fingerprint": kek},
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
    api: Option<Value>,
    #[serde(default)]
    base_url: Option<Value>,
    #[serde(default)]
    models: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    caps: Option<serde_json::Map<String, Value>>,
}

/// `PUT /api/admin/provider {api?, base_url?, models?, caps?}`: each field given
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
    if let Some(a) = b.api {
        changes.push((Setting::Api, some(a)));
    }
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

// ---------------------------------------------------------------------------
// Web tools
// ---------------------------------------------------------------------------

/// The web settings by their place in the page: (group, field, setting).
const WEB: &[(&str, &str, Setting)] = &[
    ("search", "enabled", Setting::WebSearchEnabled),
    ("search", "provider", Setting::WebSearchProvider),
    ("search", "gateway", Setting::WebSearchGateway),
    ("search", "byok_alias", Setting::WebSearchByokAlias),
    ("search", "limit", Setting::WebSearchLimit),
    ("search", "max_per_run", Setting::WebMaxSearchesPerRun),
    ("fetch", "enabled", Setting::WebFetchEnabled),
    ("fetch", "url_rule", Setting::WebFetchUrlRule),
    ("fetch", "allowlist", Setting::WebFetchAllowlist),
    ("fetch", "max_per_run", Setting::WebMaxFetchesPerRun),
    ("cloudflare", "account_id", Setting::WebCloudflareAccountId),
];

/// The web settings as `GET /api/admin/web` shows them.
///
/// # Errors
/// The store's.
pub fn web_view(st: &Store, bot: &Bot) -> clax_core::Result<Value> {
    let eff = st.effective_bot_settings()?;
    let mut groups: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut sources = serde_json::Map::new();
    for (group, field, setting) in WEB {
        let entry = eff.get(setting);
        let value = entry.map(|e| e.value.clone()).unwrap_or(Value::Null);
        let source = entry
            .map(|e| e.source.label())
            .unwrap_or_else(|| "default".into());
        groups
            .entry((*group).to_string())
            .or_insert_with(|| json!({}))[*field] = value;
        sources.insert(format!("{group}.{field}"), json!(source));
    }
    let meta = st.secret_meta(&SecretSlot::Cloudflare)?;
    sources.insert(
        "cloudflare.token".into(),
        json!(if meta.is_some() { "db" } else { "default" }),
    );
    let mut names = Names::new(st);
    groups["cloudflare"]["token"] = clax_http::bot::key_info(
        &mut names,
        meta.as_ref(),
        clax_store::secrets::CLOUDFLARE_ORIGIN,
        "the Cloudflare token",
    )?;
    let mut v = json!({"configured": crate::bot::web_configured(st, bot)?, "sources": sources});
    if let Some(why) = &bot.web_unavailable {
        v["unavailable"] = json!(why);
    }
    for (k, g) in groups {
        v[k] = g;
    }
    Ok(v)
}

/// `GET /api/admin/web` (see the module documentation).
pub async fn web(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, _) = admin(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let v = web_view(st, &bot)?;
            viewed(st, &bot, &who, &caller, "web", None)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/admin/web`'s body: each group a map of the fields it may set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebBody {
    #[serde(default)]
    search: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    fetch: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    cloudflare: Option<serde_json::Map<String, Value>>,
}

/// `PUT /api/admin/web {search?, fetch?, cloudflare?}`: each field given is
/// set (null: back to its default). 400 `unknown_setting` for a field the
/// page does not show or a value its kind refuses; 409 `setting_overridden`
/// for one the daemon's configuration sets.
pub async fn set_web(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<WebBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let mut changes: Vec<(Setting, Option<Value>)> = Vec::new();
    for (group, map) in [
        ("search", b.search),
        ("fetch", b.fetch),
        ("cloudflare", b.cloudflare),
    ] {
        for (k, v) in map.unwrap_or_default() {
            let setting = WEB
                .iter()
                .find(|(g, f, _)| *g == group && *f == k)
                .map(|(_, _, s)| *s)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "unknown_setting",
                        format!("no setting is named {group}.{k}"),
                    )
                })?;
            changes.push((setting, (!v.is_null()).then_some(v)));
        }
    }
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_bot_settings(&ctx, &changes)?;
            web_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `PUT /api/admin/web/token {token}`'s body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBody {
    token: String,
}

/// `PUT /api/admin/web/token {token}`: seals the Cloudflare API token.
pub async fn set_web_token(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<TokenBody>, JsonRejection>,
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
            st.set_secret(&ctx, kek, &SecretSlot::Cloudflare, &b.token)?;
            web_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/admin/web/token`: clears the stored Cloudflare API token.
pub async fn clear_web_token(
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
            st.clear_secret(&ctx, &SecretSlot::Cloudflare)?;
            web_view(st, &bot)
        })
        .await?;
    Ok(Json(v))
}

/// A key to send and the slot to mark when a request with it succeeds
/// (`None` for the environment's).
type KeyInUse = (ApiKey, Option<SecretSlot>);

/// The API the base URL speaks.
fn effective_api(st: &Store) -> clax_core::Result<ModelApi> {
    Ok(st
        .bot_setting(Setting::Api)?
        .as_str()
        .and_then(ModelApi::parse)
        .unwrap_or_default())
}

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
    let (key, base_url, api) = s
        .store_call(move |st| {
            let (key, base_url) = effective_key(st, &bot)?;
            Ok((key, base_url, effective_api(st)?))
        })
        .await?;
    let Some((key, slot)) = key else {
        return Ok((Err(NO_KEY.into()), None));
    };
    let store = s.store.clone();
    // The provider's request is not `Send` on the Worker's one thread.
    let r = clax_rt::sendable(async move {
        messages_client(&store)
            .list_models(api, &base_url, &key)
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

/// `GET /api/admin/provider/models` → `{models: [{id, display_name,
/// pricing}], fetched_at}`: the provider's model list with the instance key
/// and base URL in effect (502 `provider_error` when it fails, 409
/// `no_key` without a key), each with how it is priced now
/// ([`model_summary`]), recorded as `view.admin {page:
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
    let ids: Vec<String> = r
        .as_ref()
        .map(|l| l.iter().map(|m| m.id.clone()).collect())
        .unwrap_or_default();
    let (fetched_at, pricing) = s
        .store_call(move |st| {
            if let (true, Some(slot)) = (ok, &slot) {
                st.mark_secret_ok(slot)?;
            }
            viewed(st, &bot, &who, &caller, "provider.models", None)?;
            let pricing: Vec<Value> = st.model_pricing(&ids)?.iter().map(model_summary).collect();
            Ok((st.now(), pricing))
        })
        .await?;
    match r {
        Ok(list) => Ok(Json(json!({
            "models": list.iter().zip(pricing).map(|(m, p)| json!({
                "id": m.id, "display_name": m.display_name, "pricing": p,
            })).collect::<Vec<_>>(),
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

/// A model's pricing as the model picker shows it: `{source, estimated,
/// input_credits_milli_per_mtok, output_credits_milli_per_mtok}`: the
/// source its four token units share (`line`, `catalog`, `default`), else
/// `mixed`; whether any unit is priced by an estimate (the catalog or the
/// default rates); the milli-credits a million input and output tokens
/// charge now. `source` and the credits are `null` when the model resolves
/// to no price (`require_explicit_price` without its lines).
fn model_summary(p: &ModelPricing) -> Value {
    let unit = |u: Unit| {
        p.units
            .iter()
            .find(|x| x.unit == u)
            .and_then(|x| x.resolved)
    };
    let sources: Vec<Option<RateSource>> = p
        .units
        .iter()
        .map(|u| u.resolved.map(|r| r.source))
        .collect();
    let source = match sources.first() {
        Some(first) if sources.iter().all(|s| s == first) => first.map(RateSource::as_str),
        Some(_) => Some("mixed"),
        None => None,
    };
    let estimated = sources.iter().flatten().any(|s| s.is_estimated());
    let (input, output) = (unit(Unit::MtokInput), unit(Unit::MtokOutput));
    json!({
        "source": source,
        "estimated": estimated,
        "input_credits_milli_per_mtok": input.map(|r| credits_per_mtok(&r.rate)),
        "output_credits_milli_per_mtok": output.map(|r| credits_per_mtok(&r.rate)),
    })
}

// ---------------------------------------------------------------------------
// Pricing and usage
// ---------------------------------------------------------------------------

/// The default token rates, by the key `default_rates` names them, with
/// their settings.
const DEFAULT_RATES: &[(&str, Setting)] = &[
    ("input", Setting::DefaultRateInput),
    ("cached_input", Setting::DefaultRateCachedInput),
    ("cache_write", Setting::DefaultRateCacheWrite),
    ("output", Setting::DefaultRateOutput),
];

/// The key a token unit goes by in a rate card (`input`, `cached_input`,
/// `cache_write`, `output`).
fn unit_key(u: Unit) -> &'static str {
    match u {
        Unit::MtokCachedInput => "cached_input",
        Unit::MtokCacheWrite => "cache_write",
        Unit::MtokOutput => "output",
        _ => "input",
    }
}

/// Milli-credits a million tokens at `r` charge: its fixed credits, else
/// its cost and margin at its credit price, rounded up.
fn credits_per_mtok(r: &Rate) -> i64 {
    r.fixed_credits_milli.unwrap_or_else(|| {
        credits_per_unit_milli(r.cost_micro_usd, r.margin_bp, r.credit_price_micro_usd)
    })
}

/// One token unit of a model as the pricing page shows it: the rate it
/// resolves to now and where from (`source` `null`, and every rate field
/// with it, when it resolves to none), the milli-credits a million
/// tokens charge, the catalog's cost for it and whether the line's cost
/// drifted from that; `line_source`, where the cost line's figure comes
/// from as the line says (`null` without a line).
fn unit_view(u: &UnitPricing, line: Option<&CostLine>) -> Value {
    let r = u.resolved.map(|r| r.rate);
    json!({
        "source": u.resolved.map(|r| r.source.as_str()),
        "line_source": line.map(|l| l.source.as_str()),
        "cost_micro_usd": r.map(|r| r.cost_micro_usd),
        "margin_bp": r.map(|r| r.margin_bp),
        "fixed_credits_milli": r.and_then(|r| r.fixed_credits_milli),
        "applies": r.map(|r| r.applies.as_str()),
        "credits_milli_per_mtok": r.as_ref().map(credits_per_mtok),
        "catalog_micro_usd": u.catalog_micro_usd,
        "drift": u.drift,
    })
}

/// The catalog provider the API and base URL in effect name, as the
/// models are priced ([`Store::catalog_provider`]).
fn catalog_provider(st: &Store) -> clax_core::Result<Option<&'static str>> {
    st.catalog_provider()
}

/// The models the pricing page shows: the three tiers' models in effect,
/// then every other model with a token line, by ID; each with the tiers
/// it is in effect for.
fn priced_models(st: &Store) -> clax_core::Result<Vec<(String, Vec<&'static str>)>> {
    let mut out: Vec<(String, Vec<&'static str>)> = Vec::new();
    for (tier, s) in MODELS {
        let Some(m) = st.bot_setting(*s)?.as_str().map(str::to_string) else {
            continue;
        };
        match out.iter_mut().find(|(id, _)| *id == m) {
            Some((_, tiers)) => tiers.push(tier),
            None => out.push((m, vec![*tier])),
        }
    }
    let mut lined: Vec<String> = st
        .cost_lines()?
        .iter()
        .filter_map(|l| parse_token_line(&l.id).map(|(m, _)| m.to_string()))
        .filter(|m| !out.iter().any(|(id, _)| id == m))
        .collect();
    lined.sort();
    lined.dedup();
    out.extend(lined.into_iter().map(|m| (m, Vec::new())));
    Ok(out)
}

/// Model `model`'s name as its token lines' labels give it (the store
/// writes them `<name> <unit> tokens`, `Store::set_model_rates`): the
/// first of its input, cached input, cache-write and output lines whose
/// label has that form; `None` without one.
fn lines_name(lines: &[CostLine], model: &str) -> Option<String> {
    [
        (Unit::MtokInput, " input tokens"),
        (Unit::MtokCachedInput, " cached input tokens"),
        (Unit::MtokCacheWrite, " cache write tokens"),
        (Unit::MtokOutput, " output tokens"),
    ]
    .into_iter()
    .find_map(|(u, tail)| {
        let id = token_line(model, u);
        let l = lines.iter().find(|l| l.id == id)?;
        let name = l.label.strip_suffix(tail)?.trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// The pricing as `GET /api/admin/pricing` shows it.
fn pricing_view(st: &Store) -> clax_core::Result<Value> {
    // The token lines are under `models`, their rate cards: `lines` is
    // what a PUT takes back.
    let lines: Vec<Value> = st
        .cost_lines()?
        .into_iter()
        .filter(|l| parse_token_line(&l.id).is_none())
        .map(|l| {
            json!({"id": l.id, "label": l.label, "unit": l.unit.as_str(),
                   "cost_micro_usd": l.cost_micro_usd, "margin_bp": l.margin_bp,
                   "fixed_credits_milli": l.fixed_credits_milli, "applies": l.applies.as_str(),
                   "source": l.source, "model": l.model})
        })
        .collect();
    let eff = st.effective_bot_settings()?;
    let src = |s: Setting| eff.get(&s).map(|e| e.source.label());
    let get = |s: Setting| eff.get(&s).map(|e| e.value.clone()).unwrap_or(Value::Null);
    let mut sources = serde_json::Map::new();
    sources.insert(
        "credit_price_micro_usd".into(),
        json!(src(Setting::CreditPriceMicroUsd)),
    );
    sources.insert(
        "free_weekly_milli".into(),
        json!(src(Setting::FreeWeeklyMilli)),
    );
    let mut default_rates = serde_json::Map::new();
    for (key, s) in DEFAULT_RATES {
        default_rates.insert((*key).into(), get(*s));
        sources.insert(format!("default_rates.{key}"), json!(src(*s)));
    }
    sources.insert(
        "require_explicit_price".into(),
        json!(src(Setting::RequireExplicitPrice)),
    );
    let priced = priced_models(st)?;
    let ids: Vec<String> = priced.iter().map(|(m, _)| m.clone()).collect();
    let all_lines = st.cost_lines()?;
    let models: Vec<Value> = st
        .model_pricing(&ids)?
        .iter()
        .zip(&priced)
        .map(|(p, (_, tiers))| {
            let units: serde_json::Map<String, Value> = p
                .units
                .iter()
                .map(|u| {
                    let id = token_line(&p.model, u.unit);
                    let line = all_lines.iter().find(|l| l.id == id);
                    (unit_key(u.unit).to_string(), unit_view(u, line))
                })
                .collect();
            let name = lines_name(&all_lines, &p.model)
                .or_else(|| p.listing.as_ref().map(|l| l.name.clone()))
                .filter(|n| !n.is_empty());
            json!({"model": p.model, "name": name, "tiers": tiers, "units": units,
                   "listing": p.listing, "drift": p.drift})
        })
        .collect();
    Ok(json!({
        "credit_price_micro_usd": st.credit_price_micro_usd()?,
        "free_weekly_milli": st.free_weekly_milli()?,
        "token_margin_bp": TOKEN_MARGIN_BP,
        "default_rates": default_rates,
        "require_explicit_price": get(Setting::RequireExplicitPrice),
        "lines": lines,
        "models": models,
        "catalog": {
            "fetched_at": st.catalog_fetched_at()?.map(clax_core::time::stamp),
            "provider": catalog_provider(st)?,
        },
        "sources": sources,
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

/// The default token rates a `PUT /api/admin/pricing` sets; each left out
/// is kept.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultRatesBody {
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    cached_input: Option<Value>,
    #[serde(default)]
    cache_write: Option<Value>,
    #[serde(default)]
    output: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingBody {
    credit_price_micro_usd: i64,
    free_weekly_milli: i64,
    lines: Vec<LineBody>,
    #[serde(default)]
    default_rates: Option<DefaultRatesBody>,
    #[serde(default)]
    require_explicit_price: Option<Value>,
    /// What `GET` shows beside the rest; ignored.
    #[serde(default)]
    #[allow(dead_code)]
    sources: Option<Value>,
    /// What `GET` shows beside the rest; ignored.
    #[serde(default)]
    #[allow(dead_code)]
    models: Option<Value>,
    /// What `GET` shows beside the rest; ignored.
    #[serde(default)]
    #[allow(dead_code)]
    catalog: Option<Value>,
    /// What `GET` shows beside the rest; ignored.
    #[serde(default)]
    #[allow(dead_code)]
    token_margin_bp: Option<Value>,
}

/// `PUT /api/admin/pricing` (see the module documentation): the credit
/// price, the free tier, the default token rates and
/// `require_explicit_price` (each the daemon's configuration sets is
/// refused 409 `setting_overridden` when it differs), each line that is
/// not a token line made or changed, and those left out deleted (409
/// `cost_line_in_use` for one a run that has not ended may charge). Token
/// lines belong to the models' rate cards (`/api/admin/pricing/models`):
/// one sent is refused 400 `invalid_cost_line`, and every one is kept.
/// Every value is checked first and all
/// of it in one transaction ([`Store::replace_pricing`]): a refusal writes
/// nothing.
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
            let mut given = vec![
                (
                    Setting::CreditPriceMicroUsd,
                    json!(b.credit_price_micro_usd),
                ),
                (Setting::FreeWeeklyMilli, json!(b.free_weekly_milli)),
            ];
            if let Some(r) = b.default_rates {
                for (s, v) in [
                    (Setting::DefaultRateInput, r.input),
                    (Setting::DefaultRateCachedInput, r.cached_input),
                    (Setting::DefaultRateCacheWrite, r.cache_write),
                    (Setting::DefaultRateOutput, r.output),
                ] {
                    given.extend(v.map(|v| (s, v)));
                }
            }
            given.extend(
                b.require_explicit_price
                    .map(|v| (Setting::RequireExplicitPrice, v)),
            );
            // A value as it is already in effect is no change, so a page
            // that sends back what it read never trips an override.
            let mut settings = Vec::new();
            for (s, v) in given {
                if st.bot_setting(s)? != v {
                    settings.push((s, Some(v)));
                }
            }
            let bad = |what: &str| CoreError::invalid("invalid_cost_line", what.to_string());
            // Every line checked first; then everything in one transaction,
            // so a refused line writes nothing.
            let mut lines = Vec::new();
            for l in b.lines {
                let unit = Unit::parse(&l.unit).ok_or_else(|| bad("unknown unit"))?;
                let patch = CostLinePatch {
                    label: Some(l.label),
                    unit: Some(unit),
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
            // The store refuses a token line and keeps every one: rate
            // cards change only through /api/admin/pricing/models.
            st.replace_pricing(&ctx, &settings, lines)?;
            pricing_view(st)
        })
        .await?;
    Ok(Json(v))
}

/// What a rate card set by hand gives as its lines' source.
const SET_BY_ADMIN: &str = "set by an admin";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRatesBody {
    model: String,
    /// Shown in the lines' labels; the model ID by default.
    #[serde(default)]
    name: Option<String>,
    input: UnitRateBody,
    cached_input: UnitRateBody,
    cache_write: UnitRateBody,
    output: UnitRateBody,
}

/// One unit of a rate card as the route takes it: every field required,
/// `fixed_credits_milli` too (`null` for a cost-based line), so a
/// misspelt field is refused rather than read as cost-based.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitRateBody {
    cost_micro_usd: i64,
    margin_bp: i64,
    #[serde(deserialize_with = "Option::deserialize")]
    fixed_credits_milli: Option<i64>,
}

impl From<UnitRateBody> for UnitRate {
    fn from(b: UnitRateBody) -> UnitRate {
        UnitRate {
            cost_micro_usd: b.cost_micro_usd,
            margin_bp: b.margin_bp,
            fixed_credits_milli: b.fixed_credits_milli,
        }
    }
}

/// `POST /api/admin/pricing/models {model, name?, input, cached_input,
/// cache_write, output}`, each unit `{cost_micro_usd, margin_bp,
/// fixed_credits_milli}` → the pricing: sets `model`'s rate card, its four
/// token lines, each cost-based (`fixed_credits_milli` null) or
/// credit-based, labelled with `name` (left out: the name its lines have,
/// else the model ID), in one transaction, with "set by an admin" as their
/// source ([`Store::set_model_rates`]). 400 `invalid_cost_line` for a
/// model ID or a value out of range.
pub async fn set_model_rates(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<ModelRatesBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let model = b.model.trim().to_string();
    let name = b
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let mut rates = ModelRates {
        name: String::new(),
        input: b.input.into(),
        cached_input: b.cached_input.into(),
        cache_write: b.cache_write.into(),
        output: b.output.into(),
        source: SET_BY_ADMIN.into(),
    };
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            // Left out, the name its lines have, else the model ID.
            rates.name = match name {
                Some(n) => n,
                None => lines_name(&st.cost_lines()?, &model).unwrap_or_else(|| model.clone()),
            };
            st.set_model_rates(&ctx, &model, rates)?;
            pricing_view(st)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct ModelQuery {
    model: String,
}

/// `DELETE /api/admin/pricing/models?model=<id>` → the pricing: deletes
/// the model's token lines ([`Store::delete_model_rates`]), so its tokens
/// resolve to the catalog or the default rates from then on. 404
/// `not_found` when it has none; 409 `cost_line_in_use` while a run on it
/// has not ended (nothing is deleted).
pub async fn delete_model_rates(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    q: Result<Query<ModelQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.delete_model_rates(&ctx, q.model.trim())?;
            pricing_view(st)
        })
        .await?;
    Ok(Json(v))
}

/// `POST /api/admin/pricing/catalog/refresh` → the pricing: fetches the
/// price catalog now and replaces the cache with it
/// ([`crate::catalog::refresh`]), so `catalog.fetched_at` moves and the
/// models priced from it and their drift follow. 502 `tokenulator_error`
/// when it cannot be read or lists no valid price (the cache is kept).
pub async fn refresh_catalog(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    admin(&s, &d).await?;
    crate::catalog::refresh(&s).await?;
    Ok(Json(s.store_call(pricing_view).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenulatorBody {
    model: String,
    /// tokenulator.dev's provider ID; by default the one the API and base
    /// URL in effect name (`ModelApi::catalog_provider`).
    #[serde(default)]
    provider: Option<String>,
}

/// `POST /api/admin/pricing/tokenulator {model, provider?}` → the pricing:
/// refreshes the price catalog ([`crate::catalog::refresh`]), then prices
/// `model`'s input, cached input, cache-write and output lines at
/// `provider`'s listing in it (a missing cache rate at the input rate), at
/// the token margin, with tokenulator.dev and the page it read as the
/// lines' source ([`Store::price_model`]). 400 `no_provider` when no
/// provider is given and the base URL names none; 404
/// `not_on_tokenulator` when the catalog lists no such model; 502
/// `tokenulator_error` when it cannot be read and the cache does not list
/// the model either (when it does, the model is priced from the cache).
pub async fn price_from_tokenulator(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<TokenulatorBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    use clax_sample::tokenulator::LookupError;
    d.refuse_agents()?;
    admin(&s, &d).await?;
    let b = body(req)?;
    let model = b.model.trim().to_string();
    let provider = match b
        .provider
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        Some(p) => p,
        None => s
            .store_call(catalog_provider)
            .await?
            .ok_or_else(|| {
                ApiError::bad_request(
                    "no_provider",
                    "tokenulator.dev knows this provider by another name: give its provider ID",
                )
            })?
            .to_string(),
    };
    // A refresh that fails still prices from the cache when it lists the
    // model; else its refusal stands.
    let refused = crate::catalog::refresh(&s).await.err();
    let v = s
        .store_call(move |st| {
            let Some(l) = st.catalog_listing(&provider, &model)? else {
                if let Some(e) = refused {
                    return Ok(Err(e));
                }
                let e = LookupError::NotListed {
                    provider_id: provider,
                    model_id: model,
                };
                return Ok(Err(ApiError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    "not_on_tokenulator",
                    e.to_string(),
                )));
            };
            let prices = ModelPrices {
                name: if l.name.is_empty() {
                    model.clone()
                } else {
                    l.name.clone()
                },
                input: l.input_micro_usd,
                cached_input: l.cache_read_micro_usd.unwrap_or(l.input_micro_usd),
                cache_write: l.cache_write_micro_usd.unwrap_or(l.input_micro_usd),
                output: l.output_micro_usd,
                margin_bp: TOKEN_MARGIN_BP,
                source: format!(
                    "tokenulator.dev: {} {} standard rates from {}, observed {}{}",
                    l.provider_id,
                    l.model_id,
                    l.source,
                    l.observed_at,
                    match (l.cache_read_micro_usd, l.cache_write_micro_usd) {
                        (Some(_), Some(_)) => "",
                        (None, None) => " (no cache rates listed: input rate)",
                        (None, _) => " (no cache read rate listed: input rate)",
                        (_, None) => " (no cache write rate listed: input rate)",
                    }
                ),
            };
            let ctx = audit.resolve(st)?;
            st.price_model(&ctx, &model, prices)?;
            Ok(Ok(pricing_view(st)?))
        })
        .await??;
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
