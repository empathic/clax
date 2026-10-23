//! What ClaxBot's routes share (spec
//! `docs/superpowers/specs/2026-10-21-claxbot-design.md` §5, §8, §9, §10):
//! who may use ClaxBot, the admin pages and the owner's own changes
//! ([`may_admin`], [`may_use`], [`owner_only`]); the person keys and
//! actors the routes name ([`person_of`], [`Names`]); the views each GET
//! records, at most once a window per viewer and subject
//! ([`ViewLimiter`], [`record_view`]); and the state the daemon or the
//! Worker hands the routes ([`Bot`]): the key-encryption key, the
//! instance key from the environment, and the seam to whatever drives
//! runs ([`Runs`]).

use crate::auth::Caller;
use crate::error::ApiError;
use crate::identity::Identity;
use axum::http::StatusCode;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::secret::Secret;
use clax_store::Store;
use clax_store::roles::{OWNER, Role};
use clax_store::secrets::Kek;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// ClaxBot's handle, as `GET /api/agents` lists it and a send names it.
pub const HANDLE: &str = "claxbot";
/// ClaxBot's name.
pub const NAME: &str = "ClaxBot";
/// The audit trail's ID for ClaxBot, acting for anyone.
pub const ACTOR_ID: &str = "agent:claxbot";
/// The person key the store writes for Clax itself (`set_by`).
pub const SYSTEM: &str = "*system";

/// The instance key the daemon read from the environment when it
/// started (`[bot] api_key_env`), which takes precedence over a stored
/// instance key (spec §8.1).
pub struct EnvKey {
    /// The variable's name.
    pub var: String,
    pub key: Secret,
}

impl EnvKey {
    /// The key's last four characters.
    pub fn last4(&self) -> String {
        let chars: Vec<char> = self.key.expose().chars().collect();
        chars[chars.len().saturating_sub(4)..].iter().collect()
    }
}

/// What the routes tell whatever drives runs: the daemon's driver, or the
/// Worker's alarm. Every method returns at once.
pub trait Runs: Send + Sync {
    /// Run `run` was made: drive it.
    fn started(&self, _run: &str) {}
    /// A cancel of run `run` was recorded: stop it at its next boundary.
    fn cancelled(&self, _run: &str) {}
}

/// Drives nothing: runs stay queued (a daemon built without a driver,
/// and tests of the routes alone).
pub struct NoRuns;

impl Runs for NoRuns {}

/// ClaxBot's state for the routes.
pub struct Bot {
    /// Seals and opens the model keys: locally `<home>/secrets/kek`,
    /// hosted the `CLAX_KEK` secret. `None`: no key can be stored (the
    /// key routes answer 503 `no_kek`).
    pub kek: Option<Kek>,
    /// The instance key from the environment (local only).
    pub env_key: Option<EnvKey>,
    /// `[bot] enabled`: the instance runs ClaxBot at all.
    pub enabled: bool,
    /// The views recorded lately, so polling records each once a window.
    pub views: Arc<ViewLimiter>,
    pub runs: Arc<dyn Runs>,
    /// Every run's live events, which the driver emits and
    /// `GET /api/bot/runs/<id>/events` streams.
    pub events: Arc<clax_bot::EventHub>,
}

impl Default for Bot {
    fn default() -> Self {
        Bot {
            kek: None,
            env_key: None,
            enabled: true,
            views: Arc::default(),
            runs: Arc::new(NoRuns),
            events: Arc::default(),
        }
    }
}

/// A clock reading the store's (injected in tests).
pub struct StoreClock<'a>(pub &'a Store);

impl clax_core::working::Clock for StoreClock<'_> {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.now_utc()
    }
}

/// The run settings in effect on every read: the store's (the daemon's
/// `[bot]` configuration laid over them, [`configured`]), with whether
/// the instance runs ClaxBot at all.
pub struct Settings {
    pub store: Arc<Store>,
    pub enabled: bool,
}

impl clax_bot::SettingsSource for Settings {
    fn settings(&self) -> clax_core::Result<clax_bot::RunSettings> {
        Ok(clax_bot::RunSettings {
            enabled: self.enabled,
            ..clax_bot::RunSettings::from_store(&self.store)?
        })
    }
}

impl Bot {
    /// The environment's instance key where it may be sent now: while the
    /// base URL in effect is the owner's ([`Store::base_url_is_owners`]),
    /// never to one an admin set.
    ///
    /// # Errors
    /// The store's.
    pub fn env_key_for(&self, st: &Store) -> clax_core::Result<Option<&EnvKey>> {
        match &self.env_key {
            Some(k) if st.base_url_is_owners()? => Ok(Some(k)),
            _ => Ok(None),
        }
    }

    /// The model keys runs may use: the key-encryption key and the
    /// environment's instance key (which a run sends only while the base
    /// URL is the owner's).
    pub fn keys(&self) -> clax_bot::Keys {
        clax_bot::Keys {
            kek: self.kek.clone(),
            env_instance: self
                .env_key
                .as_ref()
                .map(|k| clax_sample::messages::ApiKey::new(k.key.expose())),
        }
    }

    /// Why a run for `person` may not start now (`bot.refused`'s
    /// reason), or `None` when it may: [`clax_bot::refusal`] on the
    /// settings in effect and the default model, a disabled instance
    /// reading as `not_configured`.
    ///
    /// # Errors
    /// The store's.
    pub fn refusal(&self, st: &Store, person: &str) -> clax_core::Result<Option<&'static str>> {
        let settings = clax_bot::RunSettings {
            enabled: self.enabled,
            ..clax_bot::RunSettings::from_store(st)?
        };
        let model = st
            .bot_setting(clax_store::bot_settings::Setting::ModelDefault)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let clock = StoreClock(st);
        Ok(
            clax_bot::refusal(st, &settings, &self.keys(), &clock, person, &model)?.map(
                |r| match r {
                    clax_bot::Refusal::Disabled => "not_configured",
                    other => other.code(),
                },
            ),
        )
    }

    /// The key-encryption key, or 503 `no_kek`.
    ///
    /// # Errors
    /// As above.
    pub fn kek(&self) -> Result<&Kek, ApiError> {
        self.kek.as_ref().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_kek",
                "this Clax has no key-encryption key, so it stores no model key",
            )
        })
    }
}

/// The `[bot]` keys of `config.toml` that set an instance setting, with
/// the setting each sets and the factor its value is stored at
/// (`free_weekly_credits` in credits, the store in milli-credits).
const CONFIGURED: &[(&str, clax_store::bot_settings::Setting, i64)] = {
    use clax_store::bot_settings::Setting::*;
    &[
        ("base_url", BaseUrl, 1),
        ("models.default", ModelDefault, 1),
        ("models.quick", ModelQuick, 1),
        ("models.complex", ModelComplex, 1),
        ("max_tokens", MaxTokens, 1),
        ("max_steps", MaxSteps, 1),
        ("max_run_credits", MaxRunCredits, 1),
        ("max_run_minutes", MaxRunMinutes, 1),
        ("concurrency", Concurrency, 1),
        ("script.cpu_ms", ScriptCpuMs, 1),
        ("script.memory_mib", ScriptMemoryMib, 1),
        ("script.max_calls", ScriptMaxCalls, 1),
        ("credit_price_micro_usd", CreditPriceMicroUsd, 1),
        ("free_weekly_credits", FreeWeeklyMilli, 1000),
    ]
};

/// The instance settings `cfg`'s `[bot]` sets, from the file or the
/// environment, as overrides of the stored ones
/// ([`Store::set_bot_overrides`]), and whether `[bot]` checks: an invalid
/// `[bot]` turns ClaxBot off (and sets nothing), as an invalid table turns
/// its feature off.
pub fn configured(
    cfg: &clax_core::config::HomeConfig,
) -> (clax_store::bot_settings::Overrides, bool) {
    let mut ov = clax_store::bot_settings::Overrides::default();
    let Ok(bot) = cfg.bot() else {
        return (ov, false);
    };
    for &(key, setting, factor) in CONFIGURED {
        let full = format!("bot.{key}");
        let source = cfg.source(&full);
        if source == clax_core::config::Source::Default {
            continue;
        }
        let Some(v) = cfg.value(&full) else { continue };
        let Ok(mut v) = v.deserialize::<Value>() else {
            continue;
        };
        if factor != 1 {
            v = json!(v.as_i64().unwrap_or_default().saturating_mul(factor));
        }
        if let Err(e) = ov.set(setting, &v, source.label()) {
            tracing::warn!("[bot] {key} is not taken: {e}");
        }
    }
    (ov, bot.enabled)
}

/// The person key of `caller`'s person: [`OWNER`] for the owner, a
/// connected person's public ID; `None` for anyone else.
pub fn person_of(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Owner { .. } => Some(OWNER.to_string()),
        Caller::Person { viewer } => Some(viewer.public_id.clone()),
        _ => None,
    }
}

/// `caller`'s role: the owner's, a connected person's as the store holds
/// it, anyone else a member.
///
/// # Errors
/// The store's.
pub fn role_of(st: &Store, caller: &Caller) -> clax_core::Result<Role> {
    match person_of(caller) {
        Some(p) => st.role_of(&p),
        None => Ok(Role::Member),
    }
}

/// The refusal of an agent on a route that serves people: 403
/// `not_for_agents`, for the owner's agents and a person's alike.
fn agents_refused(caller: &Caller) -> Result<(), ApiError> {
    if caller.is_agent() {
        Err(crate::auth::not_for_agents())
    } else {
        Ok(())
    }
}

/// That `caller` is a person who may use ClaxBot's own routes (`/api/me/…`,
/// runs): the owner or a connected person, as their person key. Agents get
/// 403 `not_for_agents`, no credential 401, any other viewer 403
/// `forbidden`.
///
/// # Errors
/// As above.
pub fn may_use(caller: &Caller) -> Result<String, ApiError> {
    agents_refused(caller)?;
    match (caller, person_of(caller)) {
        (_, Some(p)) => Ok(p),
        (Caller::Anonymous, None) => Err(ApiError::unauthorized()),
        _ => Err(ApiError::forbidden(
            "forbidden",
            "ClaxBot is for the owner and the people they connect",
        )),
    }
}

/// That `caller` may use the admin pages: the owner, or a person the
/// owner made an admin. Returns their role. Agents get 403
/// `not_for_agents`, no credential 401, everyone else 403 `forbidden`.
///
/// # Errors
/// As above; the store's.
pub fn may_admin(st: &Store, caller: &Caller) -> Result<Role, ApiError> {
    agents_refused(caller)?;
    if matches!(caller, Caller::Anonymous) {
        return Err(ApiError::unauthorized());
    }
    let role = role_of(st, caller)?;
    if role.is_admin() {
        Ok(role)
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            "only the owner and admins use the admin pages",
        ))
    }
}

/// 403 `owner_only` unless `role` is the owner's: only the owner grants
/// or removes admin.
///
/// # Errors
/// As above.
pub fn owner_only(role: Role) -> Result<(), ApiError> {
    clax_store::roles::check_may_grant_admin(role)
        .map_err(|_| ApiError::forbidden("owner_only", "only the owner grants or removes admin"))
}

/// The audit context of a view `who` makes, as a request that acts would
/// record it ([`Identity::audit_actor`]), through the channel its
/// credentials name. A GET route records its view with it.
///
/// # Errors
/// The store's.
pub fn view_ctx(st: &Store, who: &Identity) -> clax_core::Result<AuditCtx> {
    let actor = who.audit_actor(st, None, None)?;
    Ok(AuditCtx {
        actor,
        via: crate::audit::channel(who, None, false),
        git: clax_core::gitctx::GitField::Absent,
        call: None,
    })
}

/// The audit context of a person route's admitted `caller` (the owner or
/// a connected person; anyone else as Clax itself): the owner by the
/// owner's row (made when missing, as a browser of theirs makes it), a
/// person as the viewer they are, through the shell for a browser, else
/// the CLI.
///
/// # Errors
/// The store's.
pub fn caller_ctx(st: &Store, caller: &Caller) -> clax_core::Result<AuditCtx> {
    use clax_core::audit::{Actor, Via};
    let (actor, via) = match caller {
        Caller::Owner { browser } => (
            Actor::Owner {
                public_id: st.owner_viewer(*browser)?.public_id,
            },
            if *browser { Via::Shell } else { Via::Cli },
        ),
        Caller::Person { viewer } => (
            Actor::Viewer {
                public_id: viewer.public_id.clone(),
                display_name: viewer.display_name.clone(),
                person: true,
            },
            Via::Lan,
        ),
        _ => return Ok(AuditCtx::DAEMON),
    };
    Ok(AuditCtx {
        actor,
        via,
        git: clax_core::gitctx::GitField::Absent,
        call: None,
    })
}

/// The [`crate::settings::SettingsAudit`] that records the settings page:
/// a view as `view.admin {page: "settings"}` (once a window per viewer),
/// a write as `admin.settings {fields}` (the keys written, never their
/// values).
pub struct SettingsRecorded {
    pub views: Arc<ViewLimiter>,
}

impl crate::settings::SettingsAudit for SettingsRecorded {
    fn viewed(&self, st: &Store, who: &Caller, _fields: &[String]) {
        let viewer = viewer_key(who);
        if !self
            .views
            .admit(&format!("{viewer}\u{0}settings"), st.now_utc())
        {
            return;
        }
        let r = caller_ctx(st, who).and_then(|ctx| {
            st.record_event(
                &ctx,
                view(st, AuditKind::ViewAdmin).with("page", "settings"),
            )
        });
        if let Err(e) = r {
            tracing::warn!("the settings view was not recorded: {e}");
        }
    }

    fn changed(&self, st: &Store, who: &Caller, changes: &[clax_core::config::settings::Change]) {
        if changes.is_empty() {
            return;
        }
        let mut fields: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
        fields.sort_unstable();
        fields.dedup();
        let r = caller_ctx(st, who).and_then(|ctx| {
            st.record_event(
                &ctx,
                AuditRecord::new(AuditKind::AdminSettings, st.now()).with("fields", fields),
            )
        });
        if let Err(e) = r {
            tracing::warn!("the settings change was not recorded: {e}");
        }
    }
}

/// The views recorded lately, by viewer and subject: a view is recorded
/// once in [`ViewLimiter::WINDOW_SECS`] per viewer and subject (a page, a
/// run, a usage week, an audit filter), so a page that polls and an event
/// stream that reconnects do not flood the journal. Keys are kept as
/// SHA-256 digests (a subject may be a reader's search words), at most
/// [`ViewLimiter::CAP`] of them: when full, the ones out of their window go
/// first, then the oldest. Memory only: a restart records the next view
/// again.
#[derive(Default)]
pub struct ViewLimiter {
    seen: Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>,
}

impl ViewLimiter {
    /// The window, five minutes.
    pub const WINDOW_SECS: i64 = 300;
    /// The most keys kept.
    pub const CAP: usize = 4096;

    /// Whether a view keyed `key` at `now` is to be recorded: none was in
    /// the window before it. Recording it starts a new window.
    pub fn admit(&self, key: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        let window = chrono::Duration::seconds(Self::WINDOW_SECS);
        let key = clax_core::audit::sha256_hex(key.as_bytes());
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = seen.get(&key)
            && now - *at < window
            && now >= *at
        {
            return false;
        }
        if !seen.contains_key(&key) && seen.len() >= Self::CAP {
            seen.retain(|_, at| now - *at < window && now >= *at);
            while seen.len() >= Self::CAP {
                let Some(oldest) = seen
                    .iter()
                    .min_by_key(|(_, at)| **at)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                seen.remove(&oldest);
            }
        }
        seen.insert(key, now);
        true
    }

    /// How many keys it keeps.
    pub fn len(&self) -> usize {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether it keeps none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Who a view is by, for [`ViewLimiter`]: the person key of the owner or a
/// connected person, a viewer's public ID, else the caller's kind.
fn viewer_key(caller: &Caller) -> String {
    match caller {
        Caller::Viewer { viewer } => format!("viewer:{}", viewer.public_id),
        Caller::Anonymous => "anonymous".into(),
        Caller::OwnerAgent { session } => format!("agent:{}", session.as_deref().unwrap_or("")),
        Caller::PersonAgent { grant, .. } => format!("grant:{grant}"),
        c => person_of(c).unwrap_or_default(),
    }
}

/// Records view `rec` as `who` (keyed by `subject` for [`ViewLimiter`])
/// unless the same viewer recorded it within the window. Call it on a
/// store thread, after the view was served.
///
/// # Errors
/// The store's.
pub fn record_view(
    st: &Store,
    bot: &Bot,
    who: &Identity,
    caller: &Caller,
    subject: &str,
    rec: AuditRecord,
) -> clax_core::Result<()> {
    let key = format!(
        "{}\u{0}{}\u{0}{subject}",
        viewer_key(caller),
        rec.kind.as_str()
    );
    if !bot.views.admit(&key, st.now_utc()) {
        return Ok(());
    }
    let ctx = view_ctx(st, who)?;
    st.record_event(&ctx, rec)?;
    Ok(())
}

/// A view record of `kind` now, by the store's clock.
pub fn view(st: &Store, kind: AuditKind) -> AuditRecord {
    AuditRecord::new(kind, st.now())
}

/// Names person keys and actors for the routes, `{id, name}`, reading
/// each once.
pub struct Names<'a> {
    st: &'a Store,
    cache: HashMap<String, Option<String>>,
}

impl<'a> Names<'a> {
    pub fn new(st: &'a Store) -> Names<'a> {
        Names {
            st,
            cache: HashMap::new(),
        }
    }

    /// The display name of person key `key`: the owner's for [`OWNER`],
    /// "Clax" for [`SYSTEM`], "ClaxBot" for [`ACTOR_ID`], a viewer's by
    /// public ID; `None` when it has none.
    ///
    /// # Errors
    /// The store's.
    pub fn name(&mut self, key: &str) -> clax_core::Result<Option<String>> {
        if let Some(n) = self.cache.get(key) {
            return Ok(n.clone());
        }
        let n = match key {
            OWNER => self.st.owner()?.and_then(|v| v.display_name),
            SYSTEM => Some("Clax".into()),
            ACTOR_ID => Some(NAME.into()),
            _ => self
                .st
                .viewer_by_public_id(key)?
                .and_then(|v| v.display_name),
        };
        self.cache.insert(key.to_string(), n.clone());
        Ok(n)
    }

    /// Person key `key` as `{id, name}`.
    ///
    /// # Errors
    /// The store's.
    pub fn person(&mut self, key: &str) -> clax_core::Result<Value> {
        Ok(json!({"id": key, "name": self.name(key)?}))
    }

    /// A stored actor (the audit row's `actor`) as `{id, name}`: the owner
    /// as [`OWNER`], a viewer by public ID, ClaxBot as [`ACTOR_ID`], another
    /// agent by its handle (else its session, else `agent`), Clax itself as
    /// [`SYSTEM`], a grant's client by its name, anyone else `anonymous`.
    ///
    /// # Errors
    /// The store's.
    pub fn actor(&mut self, actor: &Value) -> clax_core::Result<Value> {
        let s = |k: &str| actor.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(match s("type").as_deref() {
            Some("owner") => self.person(OWNER)?,
            Some("viewer") => {
                let id = s("public_id").unwrap_or_default();
                let name = match s("display_name") {
                    Some(n) => Some(n),
                    None => self.name(&id)?,
                };
                json!({"id": id, "name": name})
            }
            Some("bot") => self.person(ACTOR_ID)?,
            Some("agent") => {
                let id = s("agent_handle")
                    .or_else(|| s("session_id"))
                    .unwrap_or_else(|| "agent".into());
                let name = match s("session_id") {
                    Some(sid) => self.st.get_session(&sid)?.and_then(|x| x.name),
                    None => None,
                };
                json!({"id": id, "name": name})
            }
            Some("system") => self.person(SYSTEM)?,
            Some("client") => json!({"id": s("client_name").unwrap_or_else(|| "client".into()),
                                     "name": s("client_name")}),
            _ => json!({"id": "anonymous", "name": null}),
        })
    }
}

/// A stored key's metadata as the routes show it (§8.1): never the key.
/// `None` is an empty slot.
///
/// # Errors
/// The store's.
///
/// `host` is the origin the key was set for and is only sent to; with
/// `base_url` (the provider in effect) at another, `problem` says so as
/// `<whose> was set for <host>; set it again` (`whose`: "your key", "the
/// instance key").
pub fn key_info(
    names: &mut Names<'_>,
    meta: Option<&clax_store::secrets::SecretMeta>,
    base_url: &str,
    whose: &str,
) -> clax_core::Result<Value> {
    Ok(match meta {
        None => json!({"set": false, "last4": null, "set_at": null, "set_by": null,
                       "last_ok_at": null, "host": null, "problem": null}),
        Some(m) => json!({"set": true, "last4": m.last4, "set_at": m.set_at,
                          "set_by": names.person(&m.set_by)?, "last_ok_at": m.last_ok_at,
                          "host": m.host,
                          "problem": m.host_problem(base_url).map(|p| format!("{whose}: {p}"))}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_is_admitted_once_a_window_per_key() {
        let l = ViewLimiter::default();
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-10-21T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(l.admit("a", t0));
        assert!(!l.admit("a", t0 + chrono::Duration::seconds(299)));
        assert!(l.admit("b", t0));
        assert!(l.admit("a", t0 + chrono::Duration::seconds(300)));
        // A clock that went back starts a new window rather than holding one.
        assert!(l.admit("b", t0 - chrono::Duration::seconds(10)));
    }

    #[test]
    fn the_limiter_stays_bounded_evicting_the_oldest() {
        let l = ViewLimiter::default();
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-10-21T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Every key within the window: none can be pruned as expired.
        for i in 0..ViewLimiter::CAP + 50 {
            let at = t0 + chrono::Duration::milliseconds(i as i64);
            assert!(l.admit(&format!("viewer {i} audit q=secret words"), at));
        }
        assert_eq!(l.len(), ViewLimiter::CAP);
        // The oldest went; the newest is still held.
        let late = t0 + chrono::Duration::seconds(10);
        assert!(l.admit("viewer 0 audit q=secret words", late));
        assert!(!l.admit(
            &format!("viewer {} audit q=secret words", ViewLimiter::CAP + 49),
            late
        ));
        // Keys are digests, never the raw subject.
        let seen = l.seen.lock().unwrap();
        assert!(seen.keys().all(|k| k.len() == 64 && !k.contains("secret")));
    }
}
