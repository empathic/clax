//! ClaxBot for the person (spec
//! `docs/superpowers/specs/2026-10-21-claxbot-design.md` §6.4, §7, §10):
//! their own ClaxBot state, key and usage, and runs.
//!
//! - `GET /api/me/bot` → the caller's state ([`me_view`]); `PUT
//!   /api/me/bot {use_own}`, `PUT /api/me/bot/key {key}` and `DELETE
//!   /api/me/bot/key` change their own key and answer the same. A key is
//!   write-only: no answer carries it.
//! - `GET /api/me/usage?week=<time>` → their runs and line totals of the
//!   week the time falls in (this week without it).
//! - `POST /api/bot/runs {thread_ids, artifact_id}` → `{run}`: a run of
//!   threads of an artifact, acting for the caller. Refused before any
//!   model call with 403 `no_flag`, `credits_exhausted`,
//!   `not_configured` or `model_not_priced`, recorded as `bot.refused`.
//! - `GET /api/bot/runs?artifact=<id>` → `{runs}` (no steps), `GET
//!   /api/bot/runs/<id>` → the run with its steps, `POST
//!   /api/bot/runs/<id>/cancel` → `{run}`, `GET /api/bot/runs/<id>/draft/…`
//!   → a stopped run's unpublished draft, read-only.
//!
//! Who may: the `/api/me` routes are the caller's own (the owner or a
//! connected person; agents 403 `not_for_agents`). A run is seen by the
//! person it acts for, by admins, and by anyone who may read its artifact
//! (read-only); stopped by the person it acts for and by admins; its draft
//! read by the person it acts for and by admins. Each view is recorded
//! (`view.usage`, `view.run`) at most once in five minutes per viewer and
//! subject ([`clax_http::bot::ViewLimiter`]).

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::ArtifactId;
use clax_core::CoreError;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_http::agent::DeniedToGrants;
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::bot::{Bot, Names, may_use, person_of, record_view, role_of};
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::bot_runs::{RequestKind, Run, RunStatus};
use clax_store::bot_settings::Setting;
use clax_store::credits::{KeySource, Unit};
use clax_store::roles::OWNER;
use clax_store::secrets::SecretSlot;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The typical credits of one run when there are no finished runs to
/// take it from: 5 to 25 credits, in milli-credits.
pub const DEFAULT_ESTIMATE_MILLI: (i64, i64) = (5_000, 25_000);
/// How many finished runs an estimate is taken from.
const ESTIMATE_RUNS: u32 = 20;

/// Why ClaxBot may not start a run for a person now, or `None` when it
/// may: the instance runs it, the person's flag is on, they have credits
/// left (or are unenforced), a model key is set (theirs in use, the
/// environment's, or the instance's) and the model is priced. The code is
/// the one `bot.refused` and a refused send carry; a disabled instance is
/// `not_configured`.
///
/// # Errors
/// The store's.
pub fn refusal(st: &Store, bot: &Bot, person: &str) -> clax_core::Result<Option<&'static str>> {
    bot.refusal(st, person)
}

/// What `GET /api/me/bot`'s `unavailable_reason` says for refusal `code`.
fn unavailable(code: &str) -> &str {
    match code {
        "credits_exhausted" => "no_credits",
        other => other,
    }
}

/// The model a new run takes: `models.default` in effect.
fn model_of(st: &Store) -> clax_core::Result<String> {
    Ok(st
        .bot_setting(Setting::ModelDefault)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// The key `person`'s next run would run on, or `None` when none is set:
/// their own while set and in use (with a key-encryption key to open it),
/// else the instance's (the environment's, else the stored one).
///
/// # Errors
/// The store's.
pub fn key_source(st: &Store, bot: &Bot, person: &str) -> clax_core::Result<Option<KeySource>> {
    if bot.kek.is_some()
        && st
            .secret_meta(&SecretSlot::Own(person.into()))?
            .is_some_and(|m| m.use_own)
    {
        return Ok(Some(KeySource::Own));
    }
    if bot.env_key_for(st)?.is_some()
        || (bot.kek.is_some() && st.secret_meta(&SecretSlot::Instance)?.is_some())
    {
        return Ok(Some(KeySource::Instance));
    }
    Ok(None)
}

/// The 25th and 75th percentiles of `credits` (nearest rank), or `None`
/// for none.
fn quartiles(mut credits: Vec<i64>) -> Option<(i64, i64)> {
    if credits.is_empty() {
        return None;
    }
    credits.sort_unstable();
    let at = |p: usize| credits[((credits.len() - 1) * p).div_ceil(100)];
    Some((at(25), at(75)))
}

/// The typical credits of one run for `person`: from their last finished
/// runs, else the instance's, else [`DEFAULT_ESTIMATE_MILLI`].
fn estimate(st: &Store, person: &str) -> clax_core::Result<(i64, i64)> {
    if let Some(q) = quartiles(st.finished_run_credits(Some(person), ESTIMATE_RUNS)?) {
        return Ok(q);
    }
    Ok(quartiles(st.finished_run_credits(None, ESTIMATE_RUNS)?).unwrap_or(DEFAULT_ESTIMATE_MILLI))
}

/// Whether `caller` may export the Toolpath history (`GET
/// /api/toolpath/export`): the owner, and admins.
pub fn can_export(st: &Store, who: &Identity, caller: &Caller) -> clax_core::Result<bool> {
    Ok(super::toolpath::reads_history(who) || role_of(st, caller)?.is_admin())
}

/// `GET /api/me/bot`'s answer for `person`.
///
/// # Errors
/// The store's.
pub fn me_view(
    st: &Store,
    bot: &Bot,
    who: &Identity,
    caller: &Caller,
    person: &str,
) -> clax_core::Result<Value> {
    let flags = st.flags_of(person)?;
    let refused = refusal(st, bot, person)?;
    let own = st.secret_meta(&SecretSlot::Own(person.into()))?;
    let base_url = st
        .bot_setting(clax_store::bot_settings::Setting::BaseUrl)?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut names = Names::new(st);
    let (low, high) = estimate(st, person)?;
    let mut v = json!({
        "enabled": flags.claxbot,
        "unenforced": flags.claxbot_unenforced,
        "available": refused.is_none(),
        "role": role_of(st, caller)?.as_str(),
        "can_export": can_export(st, who, caller)?,
        "estimate_milli": {"low": low, "high": high},
        // The origin a key set now is bound to, and is only ever sent to.
        "provider_host": clax_core::config::key_host(&base_url),
        "key": {
            "own": match &own {
                Some(m) => clax_http::bot::key_info(&mut names, Some(m), &base_url, "your key")?,
                None => Value::Null,
            },
            "use_own": own.as_ref().is_some_and(|m| m.use_own),
        },
        "week": st.week_usage_now(person)?,
    });
    if let Some(code) = refused {
        v["unavailable_reason"] = json!(unavailable(code));
    }
    Ok(v)
}

/// The caller's person key on a `/api/me` route.
async fn me(s: &AppState, d: &DeniedToGrants) -> Result<(Caller, String), ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(s).await?;
    let person = may_use(&caller)?;
    Ok((caller, person))
}

/// `GET /api/me/bot` (see the module documentation).
pub async fn me_get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = me(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| me_view(st, &bot, &who, &caller, &person))
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseOwnBody {
    use_own: bool,
}

/// `PUT /api/me/bot {use_own}`: whether the caller's runs use their own
/// key (404 when they have none).
pub async fn me_put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<UseOwnBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = me(&s, &d).await?;
    let b = body(req)?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.set_use_own(&ctx, &person, b.use_own)?;
            me_view(st, &bot, &who, &caller, &person)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBody {
    key: String,
}

impl KeyBody {
    /// The key given.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

/// `PUT /api/me/bot/key {key}`: seals the caller's own model key, in use
/// from now on.
pub async fn key_put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<KeyBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = me(&s, &d).await?;
    let b = body(req)?;
    let bot = s.bot.clone();
    bot.kek()?;
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            let kek = bot.kek.as_ref().ok_or(CoreError::NotFound)?;
            st.set_secret(&ctx, kek, &SecretSlot::Own(person.clone()), &b.key)?;
            me_view(st, &bot, &who, &caller, &person)
        })
        .await?;
    Ok(Json(v))
}

/// `DELETE /api/me/bot/key`: clears the caller's own model key.
pub async fn key_delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    _o: SameOrigin,
    audit: DeferredAudit,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = me(&s, &d).await?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let ctx = audit.resolve(st)?;
            st.clear_secret(&ctx, &SecretSlot::Own(person.clone()))?;
            me_view(st, &bot, &who, &caller, &person)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WeekQuery {
    /// Any time in the week (RFC 3339, or `YYYY-MM-DD`); this week without.
    #[serde(default)]
    week: Option<String>,
}

impl WeekQuery {
    /// The time asked for, as given.
    pub(crate) fn week(&self) -> Option<&str> {
        self.week.as_deref()
    }
}

/// The time `q` names, else now by the store's clock.
///
/// # Errors
/// 400 `invalid_week` for a time that does not parse.
pub fn week_at(st: &Store, q: Option<&str>) -> Result<chrono::DateTime<chrono::Utc>, ApiError> {
    let Some(raw) = q.map(str::trim).filter(|w| !w.is_empty()) else {
        return Ok(st.now_utc());
    };
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_week",
                "week is an RFC 3339 time or a YYYY-MM-DD date",
            )
        })
}

/// The week `at` falls in, `{start, end}`.
pub fn week_of(at: chrono::DateTime<chrono::Utc>) -> Value {
    let start = clax_store::credits::week_start(at);
    let fmt = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT00:00:00Z").to_string();
    json!({"start": fmt(start), "end": fmt(clax_store::credits::week_end(start))})
}

/// The label and unit of each cost line, by ID.
pub fn line_labels(st: &Store) -> clax_core::Result<BTreeMap<String, (String, Unit)>> {
    Ok(st
        .cost_lines()?
        .into_iter()
        .map(|l| (l.id, (l.label, l.unit)))
        .collect())
}

/// A line total `{line, label, unit, qty, credits_milli}` (with `extra`'s
/// fields).
pub fn line_total(
    labels: &BTreeMap<String, (String, Unit)>,
    line: &str,
    t: &clax_store::credits::Totals,
) -> Value {
    let (label, unit) = labels.get(line).map_or((line.to_string(), None), |(l, u)| {
        (l.clone(), Some(u.as_str()))
    });
    json!({"line": line, "label": label, "unit": unit, "qty": t.qty, "credits_milli": t.credits_milli,
           "cost_micro_usd": t.cost_micro_usd})
}

/// `GET /api/me/usage?week=` (see the module documentation).
pub async fn usage(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    q: Result<Query<WeekQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, person) = me(&s, &d).await?;
    let q = q
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let bot = s.bot.clone();
    let v = s
        .store_call(move |st| {
            let at = week_at(st, q.week.as_deref()).map_err(api_to_core)?;
            let labels = line_labels(st)?;
            let mut totals: BTreeMap<String, clax_store::credits::Totals> = BTreeMap::new();
            let mut runs = Vec::new();
            for u in st.usage_by_run(&person, at)? {
                for (line, t) in &u.lines {
                    let e = totals.entry(line.clone()).or_default();
                    e.qty += t.qty;
                    e.cost_micro_usd += t.cost_micro_usd;
                    e.credits_milli += t.credits_milli;
                    e.free_milli += t.free_milli;
                }
                let Some(rid) = &u.run_id else { continue };
                let Some((run, steps)) = st.run_with_steps(rid)? else {
                    continue;
                };
                let title = st
                    .get_artifact(&ArtifactId::parse(&run.artifact_id)?)?
                    .map(|a| a.title);
                let tokens: i64 = u
                    .lines
                    .iter()
                    .filter(|(l, _)| labels.get(*l).is_some_and(|(_, unit)| unit.is_tokens()))
                    .map(|(_, t)| t.qty)
                    .sum();
                let compute = u
                    .lines
                    .get(clax_store::credits::LINE_SCRIPT_COMPUTE)
                    .map_or(0, |t| t.qty);
                runs.push(json!({
                    "id": run.id, "artifact_id": run.artifact_id, "artifact_title": title,
                    "started_at": run.created_at, "status": run.status.as_str(),
                    "scripts": steps.iter().filter(|s| s.kind.as_str() == "script").count(),
                    "cpu_gb_s": compute as f64 / Unit::CpuGbS.base() as f64,
                    "tokens": tokens, "key_source": run.key_source.as_str(),
                    "credits_milli": u.total.credits_milli,
                    "lines": u.lines.iter().map(|(l, t)| line_total(&labels, l, t)).collect::<Vec<_>>(),
                }));
            }
            let v = json!({
                "week": week_of(at),
                "runs": runs,
                "totals": totals.iter().map(|(l, t)| line_total(&labels, l, t)).collect::<Vec<_>>(),
            });
            let week = clax_store::credits::week_key(at);
            let rec = clax_http::bot::view(st, AuditKind::ViewUsage)
                .with("subject_person", person.as_str())
                .with("week", week.as_str());
            record_view(st, &bot, &who, &caller, &format!("{person}/{week}"), rec)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

/// An [`ApiError`] carried through a store job, as a `CoreError`.
fn api_to_core(e: ApiError) -> CoreError {
    CoreError::invalid(e.code, e.message)
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// Who run access is being decided for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAccess {
    /// The person it acts for: may stop it and read its draft.
    Acting,
    /// An admin: the same.
    Admin,
    /// Someone who may read its artifact: may follow it, read-only.
    Reader,
}

impl RunAccess {
    /// Whether this access may stop the run and read its draft.
    pub fn controls(self) -> bool {
        matches!(self, RunAccess::Acting | RunAccess::Admin)
    }
}

/// Whether `caller` may read artifact `aid` (`live`: a live page, the
/// owner's alone): the owner always; a connected person by their rights
/// (on a hosted daemon, or at an artifact shared with them by link, the
/// link's too); a LAN viewer on the local daemon; a hosted guest while the
/// page is shared by link.
///
/// # Errors
/// The store's.
pub fn reads_artifact(
    st: &Store,
    hosted: bool,
    caller: &Caller,
    aid: &str,
    live: bool,
) -> clax_core::Result<bool> {
    match caller {
        Caller::Owner { .. } => Ok(true),
        _ if live => Ok(false),
        Caller::Person { viewer } => {
            let m = clax_http::hosted::MemberRights {
                rights: std::sync::Arc::new(st.person_rights(&viewer.public_id)?),
                person: viewer.public_id.clone(),
            };
            m.allows_at(st, (clax_core::rights::RightType::Artifacts, "read"), aid)
        }
        Caller::Viewer { .. } if hosted => Ok(st.link_access(aid)?.is_some()),
        Caller::Viewer { .. } => Ok(true),
        _ => Ok(false),
    }
}

/// What `caller` may do with `run`, or 404 when they may not see it (a
/// run is not known to exist to someone who may not).
///
/// # Errors
/// As above; the store's.
pub fn run_access(
    st: &Store,
    s_hosted: bool,
    live: bool,
    caller: &Caller,
    run: &Run,
) -> clax_core::Result<RunAccess> {
    if person_of(caller).as_deref() == Some(run.person.as_str()) {
        return Ok(RunAccess::Acting);
    }
    if role_of(st, caller)?.is_admin() {
        return Ok(RunAccess::Admin);
    }
    if reads_artifact(st, s_hosted, caller, &run.artifact_id, live)? {
        return Ok(RunAccess::Reader);
    }
    Err(CoreError::NotFound)
}

/// The caller of a run route: not an agent, not anonymous.
async fn run_caller(s: &AppState, d: &DeniedToGrants) -> Result<Caller, ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(s).await?;
    if caller.is_agent() {
        return Err(clax_http::auth::not_for_agents());
    }
    if matches!(caller, Caller::Anonymous) {
        return Err(ApiError::unauthorized());
    }
    Ok(caller)
}

/// Where a run's unpublished draft is read.
pub fn draft_url(run: &str) -> String {
    format!("/api/bot/runs/{run}/draft/")
}

/// Run `run` as the routes show it, without its steps.
///
/// # Errors
/// The store's.
pub fn run_view(st: &Store, run: &Run) -> clax_core::Result<Value> {
    let (mut input, mut cached, mut output) = (0i64, 0i64, 0i64);
    for r in st.usage_of_run(&run.id)? {
        match r.unit {
            Unit::MtokInput | Unit::MtokCacheWrite => input += r.qty,
            Unit::MtokCachedInput => cached += r.qty,
            Unit::MtokOutput => output += r.qty,
            _ => {}
        }
    }
    let draft = (matches!(run.status, RunStatus::Stopped | RunStatus::GaveUp)
        && !st.draft_files(&run.id)?.is_empty())
    .then(|| json!({"url": draft_url(&run.id)}));
    let mut names = Names::new(st);
    Ok(json!({
        "id": run.id, "artifact_id": run.artifact_id, "thread_ids": run.thread_ids,
        "request": run.request.as_str(), "status": run.status.as_str(), "reason": run.reason,
        "model": run.model, "key_source": run.key_source.as_str(),
        "acting_for": names.person(&run.person)?,
        "started_at": run.created_at, "ended_at": run.ended_at,
        "credits_milli": run.credits_milli,
        "tokens": {"input": input, "cached": cached, "output": output},
        "draft": draft,
        "last_event_id": last_event_id(st, run)?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunsQuery {
    artifact: String,
}

/// The most runs `GET /api/bot/runs` lists.
const RUNS_LISTED: u32 = 50;

/// `GET /api/bot/runs?artifact=` (see the module documentation): the
/// runs on an artifact the caller may read, newest first.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    q: Result<Query<RunsQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let caller = run_caller(&s, &d).await?;
    let q = q
        .map(|Query(q)| q)
        .map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let id = ArtifactId::parse(&q.artifact)
        .map_err(|_| ApiError::bad_request("invalid_artifact", "not an artifact ID"))?;
    let live = s.live_ids.contains(id.as_str());
    let hosted = s.hosted.is_some();
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let admin = role_of(st, &caller)?.is_admin();
            if !admin && !reads_artifact(st, hosted, &caller, id.as_str(), live)? {
                return Err(CoreError::NotFound);
            }
            let runs = st
                .runs_for_artifact(id.as_str(), RUNS_LISTED)?
                .iter()
                .map(|r| run_view(st, r))
                .collect::<clax_core::Result<Vec<_>>>()?;
            Ok(json!({"runs": runs}))
        })
        .await?;
    Ok(Json(v))
}

/// `GET /api/bot/runs/<id>` (see the module documentation).
pub async fn get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let caller = run_caller(&s, &d).await?;
    let rid = path(p)?;
    let (hosted, live_ids, bot) = (s.hosted.is_some(), s.live_ids.clone(), s.bot.clone());
    let v = s
        .store_call(move |st| {
            let (run, steps) = st.run_with_steps(&rid)?.ok_or(CoreError::NotFound)?;
            let live = live_ids.contains(&run.artifact_id);
            let access = run_access(st, hosted, live, &caller, &run)?;
            let mut v = run_view(st, &run)?;
            let mut steps = steps_view(st, &run.id, &steps)?;
            if access == RunAccess::Reader {
                steps.iter_mut().for_each(for_reader);
            }
            v["steps"] = json!(steps);
            let rec = clax_http::bot::view(st, AuditKind::ViewRun).with("run", run.id.as_str());
            record_view(st, &bot, &who, &caller, &run.id, rec)?;
            Ok(v)
        })
        .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartBody {
    thread_ids: Vec<String>,
    artifact_id: String,
}

/// Who asks for a run: the person it acts for (a person key), and, when
/// an agent of another machine asked (a grant's agent, an agent token),
/// that agent's grant, whose rights bound the run's ([`Run::agent_grant`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    pub person: String,
    pub agent_grant: Option<String>,
}

/// The sender `caller` is: the owner for the owner and the owner's agents
/// on this machine (which hold the owner's rights already), a connected
/// person for them, a person's agent of another machine as its person
/// with its grant; `None` for anyone else.
pub fn sender_of(caller: &Caller) -> Option<Sender> {
    let person = acting_for(caller)?;
    let agent_grant = match caller {
        Caller::PersonAgent { grant, .. } => Some(grant.clone()),
        _ => None,
    };
    Some(Sender {
        person,
        agent_grant,
    })
}

/// The `run_active` refusal of a send while run `id` works on one of its
/// threads: 409 with `run: {id}` (`clax_http::error`).
pub fn run_active(id: &str) -> CoreError {
    CoreError::invalid("run_active", id.to_string())
}

/// Starts a run of `request` on artifact `aid` for `sender`, as `ctx`:
/// 409 `run_active` (with its ID) while a run that has not ended names
/// one of `thread_ids`, so one thread has one active run and a second send
/// charges nothing; refused before any model call with the refusal's code
/// (403 class, recorded as `bot.refused {person, reason, artifact}` under
/// `ctx`); else made queued at the artifact's current version. The caller
/// tells the driver ([`clax_http::bot::Runs::started`]) once it returns.
///
/// # Errors
/// As above; `NotFound` for no such artifact; the store's.
pub fn start_run(
    st: &Store,
    bot: &Bot,
    ctx: &AuditCtx,
    sender: &Sender,
    aid: &ArtifactId,
    request: RequestKind,
    thread_ids: Vec<String>,
) -> clax_core::Result<Run> {
    let person = sender.person.as_str();
    let art = st.get_artifact(aid)?.ok_or(CoreError::NotFound)?;
    if let Some(active) = st.active_run_on(&thread_ids)? {
        return Err(run_active(&active.id));
    }
    if let Some(code) = refusal(st, bot, person)? {
        st.record_refused(ctx, person, code, Some(aid.as_str()))?;
        return Err(refused_error(st, person, code)?);
    }
    let key_source = key_source(st, bot, person)?.unwrap_or(KeySource::Instance);
    st.create_run_exclusive(
        ctx,
        clax_store::bot_runs::NewRun {
            artifact_id: aid.as_str().to_string(),
            person: person.to_string(),
            request,
            thread_ids,
            model: model_of(st)?,
            key_source,
            base_version: art.current_version,
            agent_grant: sender.agent_grant.clone(),
        },
    )?
    .map_err(|active| run_active(&active.id))
}

/// What a person's comment on a thread ClaxBot answered does (spec
/// §6.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowUp {
    /// A follow-up run of the thread, acting for the commenter.
    Started(String),
    /// A run works on the thread now (its ID): the commenter is queued
    /// (`Store::queue_follow_up_if_active`) for a run of their own once it ends
    /// ([`after_run`]).
    Queued(String),
}

/// The follow-up a comment by `caller` (the owner or a connected person
/// in a browser or by hand, never an agent) on thread `tid` of `aid`
/// starts (spec §6.4): when ClaxBot answered the thread (a run named it)
/// and the commenter may send the artifact's threads
/// ([`super::people::may_send_at`]), the commenter is queued for the
/// thread, once, behind those queued before, in the transaction that finds
/// a run working on it ([`FollowUp::Queued`]); with none, a new run of the
/// thread acting for them, on their credits and rights. A refusal is
/// recorded (`bot.refused`) and starts nothing; the comment stands either
/// way.
///
/// # Errors
/// The store's.
pub fn follow_up(
    st: &Store,
    bot: &Bot,
    ctx: &AuditCtx,
    caller: &Caller,
    aid: &ArtifactId,
    tid: &str,
) -> clax_core::Result<Option<FollowUp>> {
    if !matches!(caller, Caller::Owner { .. } | Caller::Person { .. }) {
        return Ok(None);
    }
    let Some(sender) = sender_of(caller) else {
        return Ok(None);
    };
    if st.runs_for_thread(tid, 1)?.is_empty() || !super::people::may_send_at(st, caller, aid)? {
        return Ok(None);
    }
    // Queued while a run works on the thread, else a run now; a run that
    // starts or ends between the two is seen at the next try.
    for _ in 0..3 {
        if let Some(active) = st.queue_follow_up_if_active(tid, &sender.person)? {
            return Ok(Some(FollowUp::Queued(active.id)));
        }
        match start_run(
            st,
            bot,
            ctx,
            &sender,
            aid,
            RequestKind::Thread,
            vec![tid.to_string()],
        ) {
            Ok(run) => return Ok(Some(FollowUp::Started(run.id))),
            Err(CoreError::Invalid {
                code: "run_active", ..
            }) => {}
            Err(e) if is_refusal(&e) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Whether `e` is a refusal [`start_run`] recorded as `bot.refused`.
fn is_refusal(e: &CoreError) -> bool {
    matches!(
        e,
        CoreError::Invalid {
            code: "no_flag" | "credits_exhausted" | "not_configured" | "model_not_priced",
            ..
        }
    )
}

/// What a run's end starts (called once it has ended, on a store
/// thread): [`start_next_follow_up`] on each of its threads. Returns the
/// runs started.
///
/// # Errors
/// The store's.
pub fn after_run(st: &Store, bot: &Bot, run: &Run) -> clax_core::Result<Vec<String>> {
    let mut started = Vec::new();
    if !run.status.is_terminal() {
        return Ok(started);
    }
    for tid in &run.thread_ids {
        started.extend(start_next_follow_up(st, bot, tid)?);
    }
    Ok(started)
}

/// The follow-ups a driver starts when it starts: [`start_next_follow_up`]
/// on every thread with people queued and no run working on it (a run
/// whose end was committed but whose follow-ups were not started, as a
/// crash between the two leaves it). Returns the runs started.
///
/// # Errors
/// The store's.
pub fn drain_follow_ups(st: &Store, bot: &Bot) -> clax_core::Result<Vec<String>> {
    let mut started = Vec::new();
    for tid in st.waiting_follow_up_threads()? {
        started.extend(start_next_follow_up(st, bot, &tid)?);
    }
    Ok(started)
}

/// Starts the follow-up of thread `tid` when no run works on it: the
/// first person queued ([`follow_up`]) who may still send and is not
/// refused gets a run of the thread acting for them alone, on their
/// credits and rights (only their comments are `from_sender`); the rest
/// stay queued, each answered when the run before ends. A person who may
/// no longer send is dropped; a refusal is recorded (`bot.refused`) and
/// the next person tried. A thread that can no longer be sent (deleted,
/// resolved, of a deleted artifact: [`check_threads`]) has its queue
/// dropped. A person stays queued until their run is made, so a run that
/// takes the thread meanwhile finds them at its end.
///
/// # Errors
/// The store's.
pub fn start_next_follow_up(st: &Store, bot: &Bot, tid: &str) -> clax_core::Result<Option<String>> {
    let Some(thread) = st.get_thread(tid)? else {
        st.drop_follow_ups(tid)?;
        return Ok(None);
    };
    let aid = ArtifactId::parse(&thread.artifact_id)?;
    if check_threads(st, &aid, &[tid.to_string()]).is_err() {
        st.drop_follow_ups(tid)?;
        return Ok(None);
    }
    while let Some((seq, person)) = st.next_follow_up(tid)? {
        if st.active_run_on(&[tid.to_string()])?.is_some() {
            return Ok(None);
        }
        let caller = crate::bot::person_caller(st, &person)?;
        let sender = caller.as_ref().and_then(sender_of);
        let (Some(caller), Some(sender)) = (caller, sender) else {
            st.remove_follow_up(seq)?;
            continue;
        };
        if !super::people::may_send_at(st, &caller, &aid)? {
            st.remove_follow_up(seq)?;
            continue;
        }
        let ctx = clax_http::bot::caller_ctx(st, &caller)?;
        match start_run(
            st,
            bot,
            &ctx,
            &sender,
            &aid,
            RequestKind::Thread,
            vec![tid.to_string()],
        ) {
            Ok(r) => {
                st.remove_follow_up(seq)?;
                return Ok(Some(r.id));
            }
            // Another run took the thread meanwhile: its end answers them.
            Err(CoreError::Invalid {
                code: "run_active", ..
            }) => return Ok(None),
            Err(e) if is_refusal(&e) => st.remove_follow_up(seq)?,
            Err(CoreError::NotFound) => {
                st.drop_follow_ups(tid)?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// ClaxBot as `GET /api/agents` lists it for `person` (spec §6.4): the
/// built-in agent, `live` when a run for them may start now, else with
/// why not (`unavailable`: `no_flag`, `no_credits`, `not_configured`,
/// `model_not_priced`). The fields every agent has are there, as an idle
/// agent's.
///
/// # Errors
/// The store's.
pub fn roster_entry(st: &Store, bot: &Bot, person: &str) -> clax_core::Result<Value> {
    let refused = refusal(st, bot, person)?;
    Ok(json!({
        "handle": clax_http::bot::HANDLE, "name": clax_http::bot::NAME,
        "harness": clax_http::bot::HANDLE, "builtin": true, "person": null, "project": null,
        "live": refused.is_none(), "unavailable": refused.map(unavailable),
        "state": "idle", "since": null, "blocked": null, "threads_owned": null,
        "working": [], "unread": null,
    }))
}

/// The error a refusal `code` for `person` answers (403 class, the code
/// its own).
///
/// # Errors
/// The store's.
pub fn refused_error(st: &Store, person: &str, code: &'static str) -> clax_core::Result<CoreError> {
    let message = match code {
        "no_flag" => "ClaxBot is not turned on for you: an admin can turn it on".to_string(),
        "credits_exhausted" => {
            let w = st.week_usage_now(person)?;
            format!("this week's credits are used up; they reset at {}", w.end)
        }
        "model_not_priced" => {
            "ClaxBot's model has no price yet: an admin sets one under Pricing".to_string()
        }
        _ => "ClaxBot is not set up on this Clax: no model key".to_string(),
    };
    Ok(CoreError::invalid(code, message))
}

/// The person a run `caller` asks for acts for: the owner for the owner
/// and the owner's agents (a grant of the owner's row included), a
/// connected person for them and their agents; `None` for anyone else.
pub fn acting_for(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Owner { .. } | Caller::OwnerAgent { .. } => Some(OWNER.into()),
        Caller::PersonAgent { person, .. } => {
            Some(person.clone().unwrap_or_else(|| OWNER.to_string()))
        }
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        Caller::Viewer { .. } | Caller::Anonymous => None,
    }
}

/// That every one of `thread_ids` is an open thread of artifact `aid`.
///
/// # Errors
/// 400 `invalid_args` for none; `NotFound` for a thread of another
/// artifact or none; `thread_resolved` for a resolved one.
pub fn check_threads(st: &Store, aid: &ArtifactId, thread_ids: &[String]) -> clax_core::Result<()> {
    if thread_ids.is_empty() || thread_ids.len() > 50 {
        return Err(CoreError::invalid(
            "invalid_args",
            "a run takes 1 to 50 threads of its artifact",
        ));
    }
    for tid in thread_ids {
        match st.get_thread(tid)? {
            Some(t) if t.artifact_id == aid.as_str() => {
                if t.status == "resolved" {
                    return Err(CoreError::invalid(
                        "thread_resolved",
                        "a resolved thread cannot be sent; add a comment to reopen it",
                    ));
                }
            }
            _ => return Err(CoreError::NotFound),
        }
    }
    Ok(())
}

/// `POST /api/bot/runs {thread_ids, artifact_id}` (see the module
/// documentation): the browser's send to ClaxBot.
pub async fn create(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    req: Result<Json<StartBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(&s).await?;
    let person = may_use(&caller)?;
    let b = body(req)?;
    let id = ArtifactId::parse(&b.artifact_id)
        .map_err(|_| ApiError::bad_request("invalid_artifact", "not an artifact ID"))?;
    let live = s.live_ids.contains(id.as_str());
    clax_http::seats::require_seat(&s, &caller, &id).await?;
    let (hosted, bot) = (s.hosted.is_some(), s.bot.clone());
    let run = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            if !reads_artifact(st, hosted, &caller, id.as_str(), live)? {
                return Err(CoreError::NotFound);
            }
            if !super::people::may_send_at(st, &caller, &id)? {
                return Err(CoreError::invalid(
                    "not_permitted",
                    "you may not send this artifact's threads",
                ));
            }
            check_threads(st, &id, &b.thread_ids)?;
            let ctx = audit.resolve(st)?;
            let sender = Sender {
                person,
                agent_grant: None,
            };
            let run = start_run(
                st,
                &bot,
                &ctx,
                &sender,
                &id,
                RequestKind::Thread,
                b.thread_ids,
            )?;
            run_view(st, &run)
        })
        .await?;
    if let Some(rid) = run["id"].as_str() {
        s.bot.runs.started(rid);
    }
    Ok((StatusCode::CREATED, Json(json!({"run": run}))).into_response())
}

/// `POST /api/bot/runs/<id>/cancel` (see the module documentation).
pub async fn cancel(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let caller = run_caller(&s, &d).await?;
    let rid = path(p)?;
    let (hosted, live_ids) = (s.hosted.is_some(), s.live_ids.clone());
    let v = s
        .store_call(move |st| {
            let run = st.run(&rid)?.ok_or(CoreError::NotFound)?;
            let live = live_ids.contains(&run.artifact_id);
            if !run_access(st, hosted, live, &caller, &run)?.controls() {
                return Err(CoreError::invalid(
                    "not_permitted",
                    "only the person this run acts for, or an admin, stops it",
                ));
            }
            let ctx = audit.resolve(st)?;
            let run = st.cancel_run(&ctx, &run.id)?;
            run_view(st, &run)
        })
        .await?;
    if let Some(rid) = v["id"].as_str() {
        s.bot.runs.cancelled(rid);
    }
    Ok(Json(json!({"run": v})))
}

/// The content security policy of a draft's files: they run sandboxed,
/// never same-origin with the API.
const DRAFT_POLICY: &str = "sandbox allow-scripts allow-forms allow-modals allow-popups";

/// `GET /api/bot/runs/<id>/draft/` and `…/draft/<path>`: a run's draft
/// (its writes over the version it was based on), read-only, for the
/// person it acts for and admins; served sandboxed, as page content on the
/// main origin is.
pub async fn draft_index(
    s: State<AppState>,
    d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let rid = path(p)?;
    serve_draft(s, d, rid, clax_core::publish::INDEX.to_string()).await
}

/// See [`draft_index`].
pub async fn draft_file(
    s: State<AppState>,
    d: DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    let (rid, file) = path(p)?;
    serve_draft(s, d, rid, file).await
}

async fn serve_draft(
    State(s): State<AppState>,
    d: DeniedToGrants,
    rid: String,
    file: String,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let caller = run_caller(&s, &d).await?;
    let (hosted, live_ids) = (s.hosted.is_some(), s.live_ids.clone());
    let found = s
        .store_call_async(move |st| async move {
            let run = st.run(&rid)?.ok_or(CoreError::NotFound)?;
            let live = live_ids.contains(&run.artifact_id);
            if !run_access(&st, hosted, live, &caller, &run)?.controls() {
                return Err(CoreError::NotFound);
            }
            if let Some(f) = st
                .draft_files(&run.id)?
                .into_iter()
                .find(|f| f.path == file)
            {
                return Ok(f.content.map(|c| {
                    let ct = f
                        .content_type
                        .unwrap_or_else(|| clax_core::publish::content_type_for(&file));
                    (c, ct)
                }));
            }
            let id = ArtifactId::parse(&run.artifact_id)?;
            Ok(st
                .read_file_async(&id, run.base_version, &file)
                .await?
                .map(|(b, m)| (b, m.content_type)))
        })
        .await?;
    let (bytes, content_type) = found.ok_or_else(ApiError::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_SECURITY_POLICY, DRAFT_POLICY.to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (
                header::HeaderName::from_static("x-content-type-options"),
                "nosniff".to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// The ID of the newest event a run's committed steps hold, as its
/// event stream numbers them (`<step>.<attempt>:<seq>`): the last
/// committed step's `step_done`, or the run's `run_end` once it ended;
/// `None` before any step is committed.
///
/// # Errors
/// The store's.
pub fn last_event_id(st: &Store, run: &Run) -> clax_core::Result<Option<String>> {
    use clax_bot::events::DONE_SEQ;
    let Some((run, steps)) = st.run_with_steps(&run.id)? else {
        return Ok(None);
    };
    Ok(steps
        .iter()
        .rev()
        .find(|s| s.committed_at.is_some())
        .map(|s| {
            let ended = s.kind.ends_as().is_some() && run.status.is_terminal();
            let seq = if ended { DONE_SEQ + 1 } else { DONE_SEQ };
            format!("{}.{}:{seq}", s.n, s.attempt)
        }))
}

/// Run `run`'s credits by step, from its usage rows.
fn credits_by_step(st: &Store, run: &str) -> clax_core::Result<BTreeMap<i64, i64>> {
    let mut out = BTreeMap::new();
    for r in st.usage_of_run(run)? {
        if let Some(n) = r.step_n {
            *out.entry(n).or_insert(0) += r.credits_milli;
        }
    }
    Ok(out)
}

/// One step as the run routes show it (spec §10): `{n, kind, status,
/// started_at, ended_at, credits_milli}` and, once committed, what its kind
/// did: a turn's narration (`text`), model, tokens and key; a script's
/// title, code, calls, logs, value, error and CPU time; a finish's
/// summary; a give-up's or a stop's reason.
pub fn step_view(s: &clax_store::bot_runs::Step, credits_milli: i64) -> Value {
    use clax_store::bot_runs::StepKind;
    let mut v = json!({
        "n": s.n, "kind": s.kind.as_str(),
        "status": if s.committed_at.is_some() { "done" } else { "running" },
        "started_at": s.started_at, "ended_at": s.committed_at, "credits_milli": credits_milli,
    });
    let Some(p) = &s.payload else {
        return v;
    };
    let o = v.as_object_mut().expect("an object");
    let mut set = |k: &str, x: Value| {
        if !x.is_null() {
            o.insert(k.to_string(), x);
        }
    };
    match s.kind {
        StepKind::Turn => {
            let text: Vec<&str> = p["assistant"]
                .as_array()
                .map(|bs| {
                    bs.iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            let u = &p["usage"];
            let n = |k: &str| u[k].as_i64().unwrap_or(0);
            set("text", json!(text.join("\n\n")));
            set("model", p["model"].clone());
            set("key_source", p["key_source"].clone());
            set(
                "input_tokens",
                json!(n("input_tokens") + n("cache_creation_input_tokens")),
            );
            set("cached_tokens", json!(n("cache_read_input_tokens")));
            set("output_tokens", json!(n("output_tokens")));
        }
        StepKind::Script => {
            let out = &p["outcome"];
            set("title", p["title"].clone());
            set("code", p["code"].clone());
            let calls: Vec<Value> = p["calls"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    json!({"op": c["op"], "target": c["target"], "ok": c["ok"].as_bool().unwrap_or(false),
                           "ms": c["ms"].as_u64().unwrap_or(0), "error": c["code"]})
                })
                .collect();
            set("calls", json!(calls));
            set("logs", out["logs"].clone());
            set("value", out["value"].clone());
            set("error", out["error"]["message"].clone());
            set("cpu_ms", json!(out["cpu_us"].as_u64().unwrap_or(0) / 1000));
        }
        StepKind::Finish => set("summary", p["summary"].clone()),
        _ => {
            let reason = if p["reason"].is_null() {
                p["code"].clone()
            } else {
                p["reason"].clone()
            };
            set("reason", reason);
        }
    }
    v
}

/// A run's steps as `GET /api/bot/runs/<id>` shows them ([`step_view`]).
///
/// # Errors
/// The store's.
pub fn steps_view(
    st: &Store,
    run: &str,
    steps: &[clax_store::bot_runs::Step],
) -> clax_core::Result<Vec<Value>> {
    let credits = credits_by_step(st, run)?;
    Ok(steps
        .iter()
        .map(|s| step_view(s, credits.get(&s.n).copied().unwrap_or(0)))
        .collect())
}

/// What of a step someone who only reads the run's artifact sees (neither
/// the person it acts for nor an admin): its place, kind, state, times,
/// credits, a script's title and its calls' ops and outcomes, a finish's
/// summary or an end's reason; never what was read with the acting
/// person's rights: narration, code, values, logs, errors, a call's target.
pub fn for_reader(step: &mut Value) {
    if let Some(o) = step.as_object_mut() {
        for k in [
            "text",
            "code",
            "value",
            "logs",
            "error",
            "input_tokens",
            "cached_tokens",
            "output_tokens",
            "key_source",
            "cpu_ms",
        ] {
            o.remove(k);
        }
        if let Some(Value::Array(calls)) = o.get_mut("calls") {
            for c in calls {
                if let Some(c) = c.as_object_mut() {
                    c.remove("target");
                    c.remove("error");
                }
            }
        }
    }
}

/// Event `data` of `event` as a reader ([`for_reader`]) sees it, or `None`
/// for one they do not see (narration as it streams).
fn event_for_reader(event: &str, mut data: Value) -> Option<Value> {
    match event {
        "turn_delta" => None,
        "script" => Some(json!({"step": data["step"], "title": data["title"]})),
        "call" => {
            if let Some(c) = data["call"].as_object_mut() {
                c.remove("target");
                c.remove("error");
            }
            Some(data)
        }
        "step_started" | "step_done" => {
            for_reader(&mut data["step"]);
            Some(data)
        }
        _ => Some(data),
    }
}

/// How often an event stream that sends nothing checks its reader may
/// still see the run; every step's end checks it too.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(180);

/// Run event `ev` as the events route sends it (spec §10), its data in
/// the shape the run panel reads: `step_started {step}`, `turn_delta
/// {step, text}` (narration only), `script {step, title, code}`, `call
/// {step, call}`, `step_done {step, run}` and `run_end {run}`, the steps
/// and the run read from the store as committed. `None` for an event the
/// panel does not show (a thinking or tool-input delta).
///
/// # Errors
/// The store's.
pub fn panel_event(
    st: &Store,
    run_id: &str,
    ev: &clax_bot::RunEvent,
) -> clax_core::Result<Option<Value>> {
    let d = &ev.data;
    let n = ev.id.step;
    Ok(Some(match ev.event {
        "step_started" => {
            let step = st
                .run_with_steps(run_id)?
                .and_then(|(_, steps)| steps.into_iter().find(|s| s.n == n))
                .map_or_else(
                    || json!({"n": n, "kind": d["kind"], "status": "running", "started_at": st.now(),
                              "credits_milli": 0}),
                    |s| {
                        // Begun (again, or replayed from its start): its
                        // content follows in the step's later events.
                        let mut v = step_view(&s, 0);
                        v["status"] = json!("running");
                        v["ended_at"] = Value::Null;
                        if let Some(o) = v.as_object_mut() {
                            for k in ["text", "calls", "logs", "value", "error", "summary", "reason"] {
                                o.remove(k);
                            }
                        }
                        v
                    },
                );
            json!({"step": step})
        }
        "turn_delta" => {
            if d["type"] != "text" {
                return Ok(None);
            }
            json!({"step": n, "text": d["text"]})
        }
        "script" => json!({"step": n, "title": d["title"], "code": d["code"]}),
        "call" => json!({"step": n, "call": {"op": d["op"], "target": d["target"], "ok": d["ok"],
                                             "ms": d["ms"].as_u64().unwrap_or(0), "error": d["code"]}}),
        "step_done" => {
            let Some((run, steps)) = st.run_with_steps(run_id)? else {
                return Ok(None);
            };
            let credits = credits_by_step(st, run_id)?;
            let step = steps
                .iter()
                .find(|s| s.n == n)
                .map(|s| step_view(s, credits.get(&s.n).copied().unwrap_or(0)))
                .unwrap_or_else(|| {
                    json!({"n": n, "kind": d["kind"], "status": "done",
                                         "credits_milli": d["credits_milli"]})
                });
            json!({"step": step, "run": run_view(st, &run)?})
        }
        "run_end" => {
            let Some(run) = st.run(run_id)? else {
                return Ok(None);
            };
            json!({"run": run_view(st, &run)?})
        }
        _ => return Ok(None),
    }))
}

/// `GET /api/bot/runs/<id>/events` (spec §10): the run's events as SSE,
/// each with its ID `<step>.<attempt>:<seq>`. With `Last-Event-ID`, every
/// event after it, then the live ones; without, from the run's first.
/// The stream ends after `run_end`, when the reader falls too far behind
/// (it reconnects with the last ID it applied), and when the daemon stops.
/// Who may: as `GET /api/bot/runs/<id>`, and a reader of the artifact
/// alone sees what [`for_reader`] leaves; access is checked again at
/// every step's end and every [`RECHECK_EVERY`] of quiet, and the stream
/// ends once it is gone. Opening it records `view.run` (at most once in
/// five minutes per viewer and run).
pub async fn events(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    headers: axum::http::HeaderMap,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let caller = run_caller(&s, &d).await?;
    let rid = path(p)?;
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(clax_bot::EventId::parse);
    let (hosted, live_ids, bot) = (s.hosted.is_some(), s.live_ids.clone(), s.bot.clone());
    let (r, c) = (rid.clone(), caller.clone());
    let (access, sub) = s
        .store_call(move |st| {
            let run = st.run(&r)?.ok_or(CoreError::NotFound)?;
            let live = live_ids.contains(&run.artifact_id);
            let access = run_access(st, hosted, live, &c, &run)?;
            let rec = clax_http::bot::view(st, AuditKind::ViewRun).with("run", run.id.as_str());
            record_view(st, &bot, &who, &c, &run.id, rec)?;
            Ok((access, bot.events.subscribe(st, &run.id, after)?))
        })
        .await?;
    struct Feed {
        s: AppState,
        sub: clax_bot::Subscription,
        rid: String,
        caller: Caller,
        reader: bool,
        checked: clax_rt::time::Instant,
        done: bool,
    }
    /// Whether `f`'s caller may still see its run.
    async fn still(f: &Feed) -> bool {
        let (rid, caller, hosted, live_ids) = (
            f.rid.clone(),
            f.caller.clone(),
            f.s.hosted.is_some(),
            f.s.live_ids.clone(),
        );
        f.s.store_call(move |st| {
            let Some(run) = st.run(&rid)? else {
                return Ok(false);
            };
            let live = live_ids.contains(&run.artifact_id);
            Ok(run_access(st, hosted, live, &caller, &run).is_ok())
        })
        .await
        .unwrap_or(false)
    }
    let feed = Feed {
        s: s.clone(),
        sub,
        rid,
        caller,
        reader: access == RunAccess::Reader,
        checked: clax_rt::time::Instant::now(),
        done: false,
    };
    let chunks = futures::stream::unfold(feed, |mut f| async move {
        loop {
            if f.done || *f.s.shutdown.borrow() {
                return None;
            }
            let keep = f.s.sse_keep_alive;
            let next = match clax_rt::time::timeout(keep, f.sub.next()).await {
                Err(_) => {
                    if f.checked.elapsed() >= RECHECK_EVERY {
                        if !still(&f).await {
                            return None;
                        }
                        f.checked = clax_rt::time::Instant::now();
                    }
                    return Some((
                        Ok::<_, std::convert::Infallible>(": keep-alive\n\n".to_string()),
                        f,
                    ));
                }
                Ok(Ok(Some(ev))) => ev,
                Ok(Ok(None)) | Ok(Err(_)) => return None,
            };
            // At a step's end, whoever reads must still be allowed to.
            if matches!(next.event, "step_done" | "run_end") {
                if !still(&f).await {
                    return None;
                }
                f.checked = clax_rt::time::Instant::now();
            }
            if next.event == "run_end" {
                f.done = true;
            }
            let (rid, ev) = (f.rid.clone(), next.clone());
            let data =
                f.s.store_call(move |st| panel_event(st, &rid, &ev))
                    .await
                    .ok()
                    .flatten();
            let data = match data {
                Some(d) if f.reader => event_for_reader(next.event, d),
                d => d,
            };
            if let Some(data) = data {
                let frame = format!("id: {}\nevent: {}\ndata: {}\n\n", next.id, next.event, data);
                return Some((Ok(frame), f));
            }
        }
    });
    let mut res = axum::body::Body::from_stream(chunks).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    h.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    h.insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::quartiles;

    #[test]
    fn estimates_are_the_middle_half_of_recent_runs() {
        assert_eq!(quartiles(vec![]), None);
        assert_eq!(quartiles(vec![7]), Some((7, 7)));
        assert_eq!(quartiles(vec![40, 10, 30, 20, 50]), Some((20, 40)));
    }
}
