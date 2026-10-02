//! The `sample` capability's daemon side (spec §6 "Sample", §9, D10): the
//! configured provider and settings. `Sampler::disabled()` has no provider:
//! pages then resolve `use("sample")` to `null` and the routes answer
//! `sampling_disabled`.

pub mod anthropic;
pub mod provider;
pub mod sse;
pub mod stub;

use clax_core::config::{HomeConfig, SampleConfig, SampleModels};
use provider::SampleProvider;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct SampleSettings {
    pub models: SampleModels,
    pub max_tokens: u32,
    pub daily_call_cap: Option<u32>,
    /// Most provider rounds of one call with tools; the last must answer.
    pub max_rounds: usize,
    /// How long a round waits for a page tool's result before sending an error result.
    pub tool_timeout: Duration,
}

impl Default for SampleSettings {
    fn default() -> Self {
        SampleSettings::from_config(&SampleConfig::default())
    }
}

impl SampleSettings {
    pub fn from_config(cfg: &SampleConfig) -> SampleSettings {
        SampleSettings {
            models: cfg.models.clone(),
            max_tokens: cfg.max_tokens,
            daily_call_cap: cfg.daily_call_cap,
            max_rounds: 5,
            tool_timeout: Duration::from_secs(150),
        }
    }
}

/// Why sampling is off on this daemon (`clax doctor` and `GET /api/sample` report it).
#[derive(Clone, Debug, PartialEq)]
pub enum OffReason {
    /// The `anthropic` provider's key variable is unset or empty.
    NoKey,
    /// `config.toml` or its `[sample]` table is invalid; the message names the problem.
    BadConfig(String),
    /// Built with [`Sampler::disabled`] (tests).
    Disabled,
}

pub struct Sampler {
    provider: Option<Arc<dyn SampleProvider>>,
    reason: Option<OffReason>,
    /// The key variable the provider reads (named in reports, never its value).
    key_env: Option<String>,
    pub settings: SampleSettings,
}

impl Sampler {
    /// Every constructor ends here, so a field added later (the pending tool
    /// calls, the cache, the counts) is initialised in one place.
    fn build(
        provider: Option<Arc<dyn SampleProvider>>,
        reason: Option<OffReason>,
        key_env: Option<String>,
        settings: SampleSettings,
    ) -> Sampler {
        Sampler {
            provider,
            reason,
            key_env,
            settings,
        }
    }

    pub fn disabled() -> Sampler {
        Sampler::build(
            None,
            Some(OffReason::Disabled),
            None,
            SampleSettings::default(),
        )
    }

    pub fn new(provider: Arc<dyn SampleProvider>, settings: SampleSettings) -> Sampler {
        Sampler::build(Some(provider), None, None, settings)
    }

    /// The provider `cfg` names. `anthropic` needs a non-empty key in the
    /// variable `cfg.api_key_env` (read through `env`), else sampling is off.
    pub fn from_config(cfg: &SampleConfig, env: impl Fn(&str) -> Option<String>) -> Sampler {
        let settings = SampleSettings::from_config(cfg);
        if cfg.provider == "stub" {
            let stub =
                stub::StubProvider::new(cfg.stub_images, Duration::from_millis(cfg.stub_delay_ms));
            return Sampler::build(Some(Arc::new(stub)), None, None, settings);
        }
        let key_env = Some(cfg.api_key_env.clone());
        match env(&cfg.api_key_env).filter(|k| !k.trim().is_empty()) {
            Some(k) => {
                let p = anthropic::AnthropicProvider::new(cfg.base_url.clone(), k);
                Sampler::build(Some(Arc::new(p)), None, key_env, settings)
            }
            None => Sampler::build(None, Some(OffReason::NoKey), key_env, settings),
        }
    }

    /// The home's `config.toml` `[sample]` table ([`HomeConfig::sample`]),
    /// read once when the daemon starts. A file or table that is invalid
    /// turns sampling off, logs one warning, and never stops the daemon.
    pub fn from_home(home_root: &std::path::Path, env: impl Fn(&str) -> Option<String>) -> Sampler {
        match HomeConfig::load(home_root).and_then(|c| c.sample()) {
            Ok(cfg) => Sampler::from_config(&cfg, env),
            Err(e) => {
                tracing::warn!(error = %e, "sample() is off");
                Sampler::build(
                    None,
                    Some(OffReason::BadConfig(e.to_string())),
                    None,
                    SampleSettings::default(),
                )
            }
        }
    }

    /// Why sampling is off; `None` when a provider is configured.
    pub fn reason(&self) -> Option<OffReason> {
        self.reason.clone()
    }

    /// The variable the `anthropic` provider reads its key from (its name only).
    pub fn key_env(&self) -> Option<&str> {
        self.key_env.as_deref()
    }

    pub fn provider(&self) -> Option<&Arc<dyn SampleProvider>> {
        self.provider.as_ref()
    }

    pub fn available(&self) -> bool {
        self.provider.is_some()
    }

    pub fn provider_name(&self) -> Option<&'static str> {
        self.provider.as_ref().map(|p| p.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::config::SampleConfig;

    #[test]
    fn a_missing_or_empty_key_disables_the_anthropic_provider() {
        let cfg = SampleConfig::default();
        assert!(!Sampler::from_config(&cfg, |_| None).available());
        assert!(!Sampler::from_config(&cfg, |_| Some(String::new())).available());
        let on = Sampler::from_config(&cfg, |k| {
            (k == "ANTHROPIC_API_KEY").then(|| "sk-test".to_string())
        });
        assert_eq!(on.provider_name(), Some("anthropic"));
        assert!(on.provider().unwrap().supports_images());
    }

    #[test]
    fn the_stub_needs_no_key_and_reports_images_as_configured() {
        let cfg = SampleConfig {
            provider: "stub".into(),
            stub_images: true,
            ..SampleConfig::default()
        };
        let s = Sampler::from_config(&cfg, |_| None);
        assert_eq!(s.provider_name(), Some("stub"));
        assert!(s.provider().unwrap().supports_images());
        assert!(Sampler::disabled().provider().is_none());
    }

    #[test]
    fn a_home_config_names_the_provider_and_a_bad_sample_table_turns_sampling_off() {
        let dir = tempfile::tempdir().unwrap();
        let write = |text: &str| std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let load = || Sampler::from_home(dir.path(), |_| None);
        assert_eq!(load().reason(), Some(OffReason::NoKey));
        write("[sample]\nprovider = \"stub\"\n");
        assert_eq!(load().provider_name(), Some("stub"));
        write("[serve]\nport = 7481\n[sample]\nprovider = \"openai\"\n");
        let off = load();
        assert!(!off.available());
        assert!(matches!(off.reason(), Some(OffReason::BadConfig(m)) if m.contains("[sample]")));
        write("[sample\n");
        assert!(matches!(load().reason(), Some(OffReason::BadConfig(_))));
    }

    #[test]
    fn settings_follow_the_config() {
        let cfg = SampleConfig {
            daily_call_cap: Some(3),
            max_tokens: 99,
            ..SampleConfig::default()
        };
        let s = SampleSettings::from_config(&cfg);
        assert_eq!(
            (s.daily_call_cap, s.max_tokens, s.max_rounds),
            (Some(3), 99, 5)
        );
        assert_eq!(s.tool_timeout, std::time::Duration::from_secs(150));
    }
}
