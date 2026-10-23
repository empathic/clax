//! ClaxBot's instance settings (migration 50, spec 2026-10-21-claxbot-design
//! §8): the provider's endpoint, the models, the caps, the credit price
//! and the free tier, as the admin pages set them. They live in the
//! database so the hosted Worker, which has no config file, reads them as
//! the daemon does; locally a value in `config.toml` or the environment
//! overrides the stored one, which is the config layer's to apply.
//!
//! Each key is known here ([`Setting`]) and its value checked; a key with
//! no row has its default ([`Setting::default_value`]). A change is
//! recorded as `admin.provider {fields[], values{key: value}}` for the
//! provider, model and cap keys (none of them a secret: the key is kept in
//! [`crate::secrets`]), and as `admin.cost_line {line: "*instance", op: "update",
//! fields}` for the credit price and the free tier.

use super::Store;
use super::roles::set_by;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::BTreeMap;

/// The cost line ID that `admin.cost_line` names for an instance-wide
/// pricing setting (the credit price, the free tier).
pub const INSTANCE_LINE: &str = "*instance";

/// Declares [`Setting`] with each key's name, kind and default.
macro_rules! settings {
    ($($v:ident = $s:literal, $kind:ident, $default:expr;)*) => {
        /// A known setting.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Setting {
            $($v,)*
        }

        impl Setting {
            /// Every setting.
            pub const ALL: &'static [Setting] = &[$(Setting::$v,)*];

            /// The setting's key, as stored and as `[bot]` in
            /// `config.toml` nests it (`models.default`).
            pub fn key(self) -> &'static str {
                match self {
                    $(Setting::$v => $s,)*
                }
            }

            fn kind(self) -> Kind {
                match self {
                    $(Setting::$v => Kind::$kind,)*
                }
            }

            /// The value of the setting with no row.
            pub fn default_value(self) -> Value {
                match self {
                    $(Setting::$v => Value::from($default),)*
                }
            }
        }
    };
}

settings! {
    BaseUrl = "base_url", Url, "https://api.anthropic.com";
    ModelDefault = "models.default", Model, "claude-sonnet-5-5";
    ModelQuick = "models.quick", Model, "claude-haiku-5-5";
    ModelComplex = "models.complex", Model, "claude-opus-5-5";
    MaxTokens = "max_tokens", Positive, 32_000;
    MaxSteps = "max_steps", Positive, 40;
    MaxRunCredits = "max_run_credits", Positive, 200;
    MaxRunMinutes = "max_run_minutes", Positive, 30;
    Concurrency = "concurrency", Positive, 4;
    ScriptCpuMs = "script.cpu_ms", Positive, 2_000;
    ScriptMemoryMib = "script.memory_mib", Positive, 64;
    ScriptMaxCalls = "script.max_calls", Positive, 200;
    CreditPriceMicroUsd = "credit_price_micro_usd", Positive, 10_000;
    FreeWeeklyMilli = "free_weekly_milli", NonNegative, 2_000_000;
}

/// What a setting's value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// An `http://` or `https://` URL with a host.
    Url,
    /// A model ID: 1 to 128 of `[A-Za-z0-9._:-]`.
    Model,
    /// An integer from 1 to 10^12.
    Positive,
    /// An integer from 0 to 10^15.
    NonNegative,
}

impl Setting {
    /// The setting with key `key`.
    ///
    /// # Errors
    /// `unknown_setting`.
    pub fn parse(key: &str) -> Result<Setting> {
        Setting::ALL
            .iter()
            .copied()
            .find(|s| s.key() == key)
            .ok_or_else(|| {
                CoreError::invalid("unknown_setting", format!("no setting is named '{key}'"))
            })
    }

    /// Whether a change to it is a pricing change (`admin.cost_line`)
    /// rather than a provider change (`admin.provider`).
    pub fn is_pricing(self) -> bool {
        matches!(
            self,
            Setting::CreditPriceMicroUsd | Setting::FreeWeeklyMilli
        )
    }

    /// `value` as the setting stores it.
    ///
    /// # Errors
    /// `invalid_setting`, naming the key and what it takes.
    pub fn check(self, value: &Value) -> Result<Value> {
        let bad = |what: &str| {
            CoreError::invalid("invalid_setting", format!("{} takes {what}", self.key()))
        };
        match self.kind() {
            Kind::Url => {
                let s = value.as_str().ok_or_else(|| bad("a URL"))?.trim();
                let what = "an https URL (http only on localhost or a loopback address), without a query or fragment";
                let u = url::Url::parse(s).map_err(|_| bad(what))?;
                // The rule `[sample]`'s base_url follows: the key goes to
                // this URL, so never in the clear beyond this machine.
                if !clax_core::config::base_url_ok(s)
                    || u.query().is_some()
                    || u.fragment().is_some()
                {
                    return Err(bad(what));
                }
                Ok(Value::from(s.trim_end_matches('/')))
            }
            Kind::Model => {
                let s = value.as_str().ok_or_else(|| bad("a model ID"))?.trim();
                let ok = !s.is_empty()
                    && s.len() <= 128
                    && s.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')
                    });
                if !ok {
                    return Err(bad("a model ID (letters, digits, '.', '_', ':' and '-')"));
                }
                Ok(Value::from(s))
            }
            Kind::Positive => match value.as_i64() {
                Some(n) if (1..=1_000_000_000_000).contains(&n) => Ok(Value::from(n)),
                _ => Err(bad("a whole number from 1")),
            },
            Kind::NonNegative => match value.as_i64() {
                Some(n) if (0..=1_000_000_000_000_000).contains(&n) => Ok(Value::from(n)),
                _ => Err(bad("a whole number from 0")),
            },
        }
    }
}

/// One setting the daemon's configuration sets: its value, checked as
/// the setting stores it, and where it came from (`file`, `env:<NAME>`).
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub value: Value,
    pub source: String,
}

/// The settings the daemon's configuration sets (`[bot]` in
/// `config.toml`, or a `CLAX_BOT__*` variable), laid over the stored ones
/// on every read: a setting here reads as its value whatever the database
/// holds, and the database's value cannot be changed while it is here
/// ([`Store::set_bot_settings`]). Empty on the hosted Worker, which has no
/// configuration of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides(BTreeMap<Setting, Override>);

/// Where ClaxBot's `base_url` may point, which decides where the instance
/// key is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BaseUrlPolicy {
    /// A local daemon: `https://`, or `http://` on a loopback host
    /// ([`clax_core::config::base_url_ok`], which [`Setting::check`]
    /// applies to every value).
    #[default]
    Local,
    /// A hosted Clax (anyone's server, or the Worker): `https://` to a
    /// public DNS name only ([`clax_core::config::hosted_base_url_ok`]).
    Hosted,
    /// A hosted Clax under test, whose scripted provider listens on a
    /// loopback `http://` port. Only a test harness sets it (the Worker's
    /// test vars, never a setting an admin reaches).
    HostedLoopbackForTests,
}

impl BaseUrlPolicy {
    /// Whether `url` may be the base URL under this policy.
    #[must_use]
    pub fn allows(self, url: &str) -> bool {
        match self {
            BaseUrlPolicy::Local => clax_core::config::base_url_ok(url),
            BaseUrlPolicy::Hosted => clax_core::config::hosted_base_url_ok(url, false),
            BaseUrlPolicy::HostedLoopbackForTests => {
                clax_core::config::hosted_base_url_ok(url, true)
            }
        }
    }
}

impl Overrides {
    /// Sets `s` to `value` from `source`.
    ///
    /// # Errors
    /// `invalid_setting` when the value does not check
    /// ([`Setting::check`]).
    pub fn set(&mut self, s: Setting, value: &Value, source: impl Into<String>) -> Result<()> {
        let value = s.check(value)?;
        self.0.insert(
            s,
            Override {
                value,
                source: source.into(),
            },
        );
        Ok(())
    }

    /// The override of `s`, when the configuration sets it.
    pub fn get(&self, s: Setting) -> Option<&Override> {
        self.0.get(&s)
    }

    /// Every override, by setting.
    pub fn iter(&self) -> impl Iterator<Item = (&Setting, &Override)> {
        self.0.iter()
    }
}

/// Where a setting's value in effect comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingSource {
    /// No layer sets it.
    Default,
    /// The admin pages set it (a row of `bot_settings`).
    Db,
    /// The daemon's configuration sets it: `file` or `env:<NAME>`.
    Config(String),
}

impl SettingSource {
    /// `default`, `db`, `file` or `env:<NAME>`.
    pub fn label(&self) -> String {
        match self {
            SettingSource::Default => "default".into(),
            SettingSource::Db => "db".into(),
            SettingSource::Config(s) => s.clone(),
        }
    }
}

/// A setting's value in effect, and where it comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub value: Value,
    pub source: SettingSource,
}

/// The value of `s` on `c` under `ov`: the override's, else its row's,
/// else its default.
pub(crate) fn setting_in(c: &Connection, ov: &Overrides, s: Setting) -> Result<Value> {
    if let Some(o) = ov.get(s) {
        return Ok(o.value.clone());
    }
    stored_in(c, s)
}

/// The value of `s` in the database, whatever overrides it: its row's,
/// else its default.
fn stored_in(c: &Connection, s: Setting) -> Result<Value> {
    let stored: Option<String> = c
        .prepare_cached("SELECT value_json FROM bot_settings WHERE key = ?1")?
        .query_row(params![s.key()], |r| r.get(0))
        .optional()?;
    Ok(stored
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_else(|| s.default_value()))
}

/// [`setting_in`] for an integer setting.
pub(crate) fn int_setting_in(c: &Connection, ov: &Overrides, s: Setting) -> Result<i64> {
    Ok(setting_in(c, ov, s)?
        .as_i64()
        .or_else(|| s.default_value().as_i64())
        .unwrap_or_default())
}

/// One stored setting: its value and who set it when.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSetting {
    pub value: Value,
    pub set_by: String,
    pub set_at: String,
}

impl Store {
    /// The stored settings by key; a key with no row is not in it.
    ///
    /// # Errors
    /// The store's.
    pub fn stored_bot_settings(&self) -> Result<BTreeMap<String, StoredSetting>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT key, value_json, set_by, set_at FROM bot_settings ORDER BY key",
            )?;
            let rows = q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            let mut out = BTreeMap::new();
            for row in rows {
                let (k, v, set_by, set_at) = row?;
                if let Ok(value) = serde_json::from_str(&v) {
                    out.insert(
                        k,
                        StoredSetting {
                            value,
                            set_by,
                            set_at,
                        },
                    );
                }
            }
            Ok(out)
        })
    }

    /// Lays `ov` over the stored settings from now on, replacing any set
    /// before: the daemon sets its `[bot]` configuration here when it
    /// starts.
    pub fn set_bot_overrides(&self, ov: Overrides) {
        *self
            .bot_overrides
            .write()
            .unwrap_or_else(|e| e.into_inner()) = std::sync::Arc::new(ov);
    }

    /// Sets where `base_url` may point from now on (a hosted server sets
    /// [`BaseUrlPolicy::Hosted`] as it starts).
    pub fn set_base_url_policy(&self, policy: BaseUrlPolicy) {
        *self
            .base_url_policy
            .write()
            .unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Where `base_url` may point ([`Store::set_base_url_policy`]).
    pub fn base_url_policy(&self) -> BaseUrlPolicy {
        *self
            .base_url_policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The overrides in effect ([`Store::set_bot_overrides`]).
    pub fn bot_overrides(&self) -> std::sync::Arc<Overrides> {
        self.bot_overrides
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The value of `s` in effect: its override, else stored, else its
    /// default.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_setting(&self, s: Setting) -> Result<Value> {
        let ov = self.bot_overrides();
        self.with_read(|c| setting_in(c, &ov, s))
    }

    /// Whether the base URL in effect is the owner's: the default or the
    /// daemon's configuration, not a row the admin pages wrote. Only then
    /// may the environment's instance key (`[bot] api_key_env`) be sent
    /// to it; with a base URL from the database the instance key is the
    /// stored one alone.
    ///
    /// # Errors
    /// The store's.
    pub fn base_url_is_owners(&self) -> Result<bool> {
        if self.bot_overrides().get(Setting::BaseUrl).is_some() {
            return Ok(true);
        }
        Ok(!self
            .stored_bot_settings()?
            .contains_key(Setting::BaseUrl.key()))
    }

    /// Every setting's value in effect and where it comes from.
    ///
    /// # Errors
    /// The store's.
    pub fn effective_bot_settings(&self) -> Result<BTreeMap<Setting, Effective>> {
        let ov = self.bot_overrides();
        let stored = self.stored_bot_settings()?;
        Ok(Setting::ALL
            .iter()
            .map(|&s| {
                let e = match (ov.get(s), stored.get(s.key())) {
                    (Some(o), _) => Effective {
                        value: o.value.clone(),
                        source: SettingSource::Config(o.source.clone()),
                    },
                    (None, Some(row)) => Effective {
                        value: row.value.clone(),
                        source: SettingSource::Db,
                    },
                    (None, None) => Effective {
                        value: s.default_value(),
                        source: SettingSource::Default,
                    },
                };
                (s, e)
            })
            .collect())
    }

    /// The micro-USD one credit is priced at.
    ///
    /// # Errors
    /// The store's.
    pub fn credit_price_micro_usd(&self) -> Result<i64> {
        let ov = self.bot_overrides();
        self.with_read(|c| int_setting_in(c, &ov, Setting::CreditPriceMicroUsd))
    }

    /// The free credits every person has each week before their lift, in
    /// milli-credits.
    ///
    /// # Errors
    /// The store's.
    pub fn free_weekly_milli(&self) -> Result<i64> {
        let ov = self.bot_overrides();
        self.with_read(|c| int_setting_in(c, &ov, Setting::FreeWeeklyMilli))
    }

    /// Sets (`Some`) or clears back to the default (`None`) each of
    /// `changes`, in one transaction, every value checked first. Records
    /// one `admin.provider {fields[]}` for the provider, model and cap keys
    /// that changed, and one `admin.cost_line {line: "*instance", op:
    /// "update", fields{key: value}}` for the pricing keys that changed.
    /// Returns the keys that changed. A setting the daemon's configuration
    /// sets ([`Store::set_bot_overrides`]) is refused, since a stored value
    /// would not take effect.
    ///
    /// # Errors
    /// `invalid_setting` for a value; `setting_overridden` for a setting
    /// the configuration sets, naming where; the store's.
    pub fn set_bot_settings(
        &self,
        ctx: &AuditCtx,
        changes: &[(Setting, Option<Value>)],
    ) -> Result<Vec<Setting>> {
        let checked = self.checked_settings(changes)?;
        self.with_tx(|tx| self.apply_settings_in(tx, ctx, &checked))
    }

    /// `changes` checked as [`Store::set_bot_settings`] checks them, before
    /// any is written.
    pub(crate) fn checked_settings(
        &self,
        changes: &[(Setting, Option<Value>)],
    ) -> Result<Vec<(Setting, Option<Value>)>> {
        let ov = self.bot_overrides();
        if let Some((s, o)) = changes
            .iter()
            .find_map(|(s, _)| ov.get(*s).map(|o| (*s, o)))
        {
            return Err(CoreError::invalid(
                "setting_overridden",
                format!(
                    "{} is set by the daemon's configuration ({}): change it there",
                    s.key(),
                    o.source
                ),
            ));
        }
        let checked: Vec<(Setting, Option<Value>)> = changes
            .iter()
            .map(|(s, v)| Ok((*s, v.as_ref().map(|v| s.check(v)).transpose()?)))
            .collect::<Result<_>>()?;
        let policy = self.base_url_policy();
        if let Some(url) = checked.iter().find_map(|(s, v)| {
            (*s == Setting::BaseUrl)
                .then(|| v.as_ref().and_then(Value::as_str))
                .flatten()
        }) && !policy.allows(url)
        {
            return Err(CoreError::invalid(
                "invalid_setting",
                "base_url takes a public https URL on a hosted Clax: no IP address, \
                 localhost or private network name",
            ));
        }
        Ok(checked)
    }

    /// Writes checked settings `checked` on `tx`, recording them as
    /// [`Store::set_bot_settings`] does.
    pub(crate) fn apply_settings_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        checked: &[(Setting, Option<Value>)],
    ) -> Result<Vec<Setting>> {
        {
            let now = self.now();
            let mut changed = Vec::new();
            for (s, v) in checked {
                let before = stored_in(tx, *s)?;
                let had_row: bool = tx
                    .query_row(
                        "SELECT count(*) FROM bot_settings WHERE key = ?1",
                        params![s.key()],
                        |r| r.get::<_, i64>(0),
                    )
                    .map(|n| n > 0)?;
                match v {
                    Some(v) => {
                        if had_row && before == *v {
                            continue;
                        }
                        tx.execute(
                            "INSERT INTO bot_settings (key, value_json, set_by, set_at)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json,
                               set_by = excluded.set_by, set_at = excluded.set_at",
                            params![s.key(), v.to_string(), set_by(ctx), now],
                        )?;
                        if before != *v {
                            changed.push(*s);
                        }
                    }
                    None => {
                        if !had_row {
                            continue;
                        }
                        tx.execute("DELETE FROM bot_settings WHERE key = ?1", params![s.key()])?;
                        if before != s.default_value() {
                            changed.push(*s);
                        }
                    }
                }
            }
            changed.sort();
            changed.dedup();
            let provider: Vec<&str> = changed
                .iter()
                .filter(|s| !s.is_pricing())
                .map(|s| s.key())
                .collect();
            // The stored instance key was set for the old URL: a new one
            // is not sent it until an admin enters the key again.
            let mut key_cleared = false;
            if changed.contains(&Setting::BaseUrl) {
                let slot = crate::secrets::SecretSlot::Instance;
                if let Some(meta) = crate::secrets::meta_in(tx, &slot)? {
                    tx.execute(
                        "DELETE FROM sealed_secrets WHERE scope = ?1 AND person = ?2",
                        params![slot.scope(), slot.person()],
                    )?;
                    let rec = crate::secrets::key_record(now.clone(), &slot, "clear", &meta.last4);
                    self.record_audit(tx, ctx, rec)?;
                    key_cleared = true;
                }
            }
            if !provider.is_empty() {
                let mut values = serde_json::Map::new();
                for s in changed.iter().filter(|s| !s.is_pricing()) {
                    values.insert(s.key().into(), stored_in(tx, *s)?);
                }
                let mut rec = AuditRecord::new(AuditKind::AdminProvider, now.clone())
                    .with("fields", provider)
                    .with("values", Value::Object(values));
                if key_cleared {
                    rec = rec.with("key_cleared", true);
                }
                self.record_audit(tx, ctx, rec)?;
            }
            let mut pricing = serde_json::Map::new();
            for s in changed.iter().filter(|s| s.is_pricing()) {
                pricing.insert(s.key().into(), stored_in(tx, *s)?);
            }
            if !pricing.is_empty() {
                let rec = AuditRecord::new(AuditKind::AdminCostLine, now)
                    .with("line", INSTANCE_LINE)
                    .with("op", "update")
                    .with("fields", Value::Object(pricing));
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, store};
    use serde_json::json;

    #[test]
    fn defaults_follow_the_spec() {
        let (_d, st) = store();
        assert_eq!(st.credit_price_micro_usd().unwrap(), 10_000);
        assert_eq!(st.free_weekly_milli().unwrap(), 2_000_000);
        assert_eq!(
            st.bot_setting(Setting::ModelDefault).unwrap(),
            json!("claude-sonnet-5-5")
        );
        assert!(st.stored_bot_settings().unwrap().is_empty());
    }

    #[test]
    fn overrides_win_on_every_read_and_refuse_writes() {
        let (_d, st) = store();
        st.set_bot_settings(
            DAEMON,
            &[
                (Setting::MaxSteps, Some(json!(12))),
                (Setting::FreeWeeklyMilli, Some(json!(5_000_000))),
            ],
        )
        .unwrap();
        let mut ov = Overrides::default();
        ov.set(Setting::FreeWeeklyMilli, &json!(7_000_000), "file")
            .unwrap();
        ov.set(
            Setting::ModelQuick,
            &json!("claude-x"),
            "env:CLAX_BOT__MODELS__QUICK",
        )
        .unwrap();
        assert!(ov.set(Setting::MaxSteps, &json!(0), "file").is_err());
        st.set_bot_overrides(ov);
        assert_eq!(st.free_weekly_milli().unwrap(), 7_000_000);
        assert_eq!(
            st.week_usage_now(crate::roles::OWNER).unwrap().weekly_milli,
            7_000_000
        );
        let eff = st.effective_bot_settings().unwrap();
        assert_eq!(eff[&Setting::MaxSteps].value, json!(12));
        assert_eq!(eff[&Setting::MaxSteps].source.label(), "db");
        assert_eq!(eff[&Setting::FreeWeeklyMilli].source.label(), "file");
        assert_eq!(
            eff[&Setting::ModelQuick].source.label(),
            "env:CLAX_BOT__MODELS__QUICK"
        );
        assert_eq!(eff[&Setting::BaseUrl].source.label(), "default");
        let refused = st.set_bot_settings(DAEMON, &[(Setting::ModelQuick, Some(json!("y")))]);
        assert!(
            matches!(
                &refused,
                Err(CoreError::Invalid { code: "setting_overridden", message })
                    if message.contains("env:CLAX_BOT__MODELS__QUICK")
            ),
            "{refused:?}"
        );
        // The stored value is kept, and reads again once the override goes.
        st.set_bot_overrides(Overrides::default());
        assert_eq!(st.free_weekly_milli().unwrap(), 5_000_000);
    }

    #[test]
    fn settings_are_checked_set_cleared_and_recorded() {
        let (_d, st) = store();
        for (s, bad) in [
            (Setting::BaseUrl, json!("ftp://x")),
            (Setting::BaseUrl, json!("http://proxy.example")),
            (Setting::BaseUrl, json!("http://10.0.0.2:8080")),
            (Setting::BaseUrl, json!("https://user@proxy.example")),
            (Setting::BaseUrl, json!("https://proxy.example/?token=x")),
            (Setting::BaseUrl, json!(3)),
            (Setting::ModelQuick, json!("a b")),
            (Setting::MaxSteps, json!(0)),
            (Setting::FreeWeeklyMilli, json!(-1)),
        ] {
            assert!(
                matches!(
                    st.set_bot_settings(DAEMON, &[(s, Some(bad.clone()))]),
                    Err(CoreError::Invalid {
                        code: "invalid_setting",
                        ..
                    })
                ),
                "{s:?} {bad}"
            );
        }
        assert!(Setting::parse("nope").is_err());
        let changed = st
            .set_bot_settings(
                DAEMON,
                &[
                    (Setting::BaseUrl, Some(json!("https://proxy.example/"))),
                    (Setting::ModelDefault, Some(json!("claude-sonnet-5-5"))),
                    (Setting::CreditPriceMicroUsd, Some(json!(20_000))),
                ],
            )
            .unwrap();
        // Storing the default is no change of value.
        assert_eq!(changed, [Setting::BaseUrl, Setting::CreditPriceMicroUsd]);
        assert_eq!(
            st.bot_setting(Setting::BaseUrl).unwrap(),
            json!("https://proxy.example")
        );
        assert_eq!(st.credit_price_micro_usd().unwrap(), 20_000);
        assert_eq!(st.stored_bot_settings().unwrap().len(), 3);
        let changed = st
            .set_bot_settings(DAEMON, &[(Setting::CreditPriceMicroUsd, None)])
            .unwrap();
        assert_eq!(changed, [Setting::CreditPriceMicroUsd]);
        assert_eq!(st.credit_price_micro_usd().unwrap(), 10_000);
        let kinds: Vec<(String, String)> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("admin."))
            .map(|e| (e.kind, e.body))
            .collect();
        assert_eq!(kinds.len(), 3, "{kinds:?}");
        assert_eq!(kinds[0].0, "admin.provider");
        assert!(
            kinds[0].1.contains("\"fields\":[\"base_url\"]"),
            "{}",
            kinds[0].1
        );
        assert!(
            kinds[0]
                .1
                .contains("\"values\":{\"base_url\":\"https://proxy.example\"}"),
            "the new value is recorded: {}",
            kinds[0].1
        );
        // http on loopback is allowed.
        st.set_bot_settings(
            DAEMON,
            &[(Setting::BaseUrl, Some(json!("http://127.0.0.1:8123")))],
        )
        .unwrap();
        assert_eq!(kinds[1].0, "admin.cost_line");
        assert!(kinds[1].1.contains("\"credit_price_micro_usd\":20000"));
        assert!(kinds[2].1.contains("\"credit_price_micro_usd\":10000"));
    }

    #[test]
    fn a_hosted_store_takes_only_a_public_https_base_url() {
        let (_d, st) = store();
        let set = |url: &str| st.set_bot_settings(DAEMON, &[(Setting::BaseUrl, Some(json!(url)))]);
        // Locally, loopback http is a provider on this machine.
        assert!(set("http://127.0.0.1:9000").is_ok());
        st.set_base_url_policy(BaseUrlPolicy::Hosted);
        for bad in [
            "http://127.0.0.1:9000",
            "https://10.0.0.5",
            "https://169.254.169.254",
            "https://[fd00::1]",
            "https://metadata.google.internal",
            "https://localhost",
        ] {
            let e = set(bad).unwrap_err();
            assert!(
                matches!(&e, CoreError::Invalid { code, .. } if *code == "invalid_setting"),
                "{bad}: {e:?}"
            );
        }
        assert!(set("https://api.anthropic.com").is_ok());
        // The test escape takes loopback http again, nothing more.
        st.set_base_url_policy(BaseUrlPolicy::HostedLoopbackForTests);
        assert!(set("http://127.0.0.1:9000").is_ok());
        assert!(set("https://10.0.0.5").is_err());
        // Clearing the URL (back to its default) is always allowed.
        st.set_base_url_policy(BaseUrlPolicy::Hosted);
        assert!(
            st.set_bot_settings(DAEMON, &[(Setting::BaseUrl, None)])
                .is_ok()
        );
    }

    #[test]
    fn a_new_base_url_clears_the_stored_instance_key() {
        use crate::secrets::{Kek, SecretSlot};
        let (_d, st) = store();
        let kek = Kek::from_bytes([7; 32]);
        st.set_secret(
            DAEMON,
            &kek,
            &SecretSlot::Instance,
            "sk-ant-api03-ABCDEFGHIJKL-wxyz",
        )
        .unwrap();
        // A change of anything else keeps it.
        st.set_bot_settings(DAEMON, &[(Setting::MaxSteps, Some(json!(12)))])
            .unwrap();
        assert!(st.secret_meta(&SecretSlot::Instance).unwrap().is_some());
        // The same URL again changes nothing and keeps it.
        st.set_bot_settings(
            DAEMON,
            &[(Setting::BaseUrl, Some(json!("https://api.anthropic.com")))],
        )
        .unwrap();
        assert!(st.secret_meta(&SecretSlot::Instance).unwrap().is_some());
        st.set_bot_settings(
            DAEMON,
            &[(Setting::BaseUrl, Some(json!("https://proxy.example.com")))],
        )
        .unwrap();
        assert!(st.secret_meta(&SecretSlot::Instance).unwrap().is_none());
        let events: Vec<(String, String)> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("admin."))
            .map(|e| (e.kind, e.body))
            .collect();
        let last = events.last().unwrap();
        assert_eq!(last.0, "admin.provider", "{events:?}");
        assert!(last.1.contains("\"key_cleared\":true"), "{}", last.1);
        assert!(
            events
                .iter()
                .any(|(k, b)| k == "admin.key" && b.contains("\"op\":\"clear\"")),
            "{events:?}"
        );
        // With no key stored, a change records no clear.
        st.set_bot_settings(
            DAEMON,
            &[(Setting::BaseUrl, Some(json!("https://other.example.com")))],
        )
        .unwrap();
        let body = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .rfind(|e| e.kind == "admin.provider")
            .unwrap()
            .body;
        assert!(!body.contains("key_cleared"), "{body}");
    }
}
