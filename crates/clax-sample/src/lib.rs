//! The `sample` capability's daemon side (spec §6 "Sample", §9, D10): the
//! configured provider and settings, the Anthropic and stub providers, a
//! call's rounds (`flight`), the quota and the answer cache, and the checks
//! of a page's request (`request`, refusing with a `SampleError` the route
//! answers 400 with). `Sampler::disabled()` has no provider: pages then
//! resolve `use("sample")` to `null` and the routes answer
//! `sampling_disabled`. It knows nothing of HTTP routes or of the store.

pub mod anthropic;
pub mod cache;
pub mod flight;
pub mod json_reply;
pub mod pricing;
pub mod provider;
pub mod quota;
pub mod request;
pub mod sse;
pub mod stub;

pub use anthropic::KeySource;
use clax_core::config::{HomeConfig, SampleConfig, SampleModels};
use provider::SampleProvider;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq)]
pub struct SampleSettings {
    pub models: SampleModels,
    pub max_tokens: u32,
    pub daily_call_cap: Option<u32>,
    /// Most micro-USD of model use per artifact per local day.
    pub daily_cost_cap: Option<u64>,
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
            daily_cost_cap: cfg.daily_cost_cap_usd.map(pricing::micro),
            max_rounds: 5,
            tool_timeout: Duration::from_secs(150),
        }
    }
}

/// Why sampling is off on this daemon (`clax doctor` and `GET /api/sample` report it).
#[derive(Clone, Debug, PartialEq)]
pub enum OffReason {
    /// The `anthropic` provider's key variable is unset or empty, or the
    /// slot it reads ([`KeySlot`]) holds no key.
    NoKey,
    /// `config.toml` or its `[sample]` table is invalid; the message names the problem.
    BadConfig(String),
    /// Built with [`Sampler::disabled`] (tests).
    Disabled,
}

/// The owner's key, held in memory by a host app's daemon and set over
/// loopback (`PUT /api/sample/key`). Never written to disk, never logged:
/// `Debug` prints whether it is set, not the key.
#[derive(Clone, Default)]
pub struct KeySlot(Arc<std::sync::RwLock<Option<String>>>);

impl KeySlot {
    pub fn new() -> KeySlot {
        KeySlot::default()
    }

    /// Sets the key; `None` or a blank key clears it.
    pub fn set(&self, key: Option<String>) {
        let key = key.filter(|k| !k.trim().is_empty());
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = key;
    }

    pub fn get(&self) -> Option<String> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn is_set(&self) -> bool {
        self.0.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }
}

impl std::fmt::Debug for KeySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySlot")
            .field("set", &self.is_set())
            .finish()
    }
}

pub struct Sampler {
    provider: Option<Arc<dyn SampleProvider>>,
    /// The slot `PUT /api/sample/key` fills, when built with one.
    slot: Option<KeySlot>,
    /// Whether the provider reads its key from [`Sampler::slot`], so an
    /// empty slot turns sampling off (`false` for the stub).
    reads_slot: bool,
    reason: Option<OffReason>,
    /// The key variable the provider reads (named in reports, never its value).
    key_env: Option<String>,
    pub settings: SampleSettings,
    /// Running calls and the tool results they wait for.
    pending: Mutex<HashMap<String, PendingCall>>,
    /// Stored answers and running flights, per viewer and artifact.
    pub cache: cache::AnswerCache,
    /// Calls per artifact per day.
    pub counts: quota::CallCounts,
    /// Running and waiting calls per viewer.
    pub queues: quota::ViewerQueues,
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
            slot: None,
            reads_slot: false,
            reason,
            key_env,
            settings,
            pending: Mutex::default(),
            cache: Default::default(),
            counts: Default::default(),
            queues: Default::default(),
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
            Err(e) => Sampler::bad_config(e),
        }
    }

    /// Sampling off for an invalid `config.toml`, with one warning.
    fn bad_config(e: impl std::fmt::Display) -> Sampler {
        tracing::warn!(error = %e, "sample() is off");
        Sampler::build(
            None,
            Some(OffReason::BadConfig(e.to_string())),
            None,
            SampleSettings::default(),
        )
    }

    /// The home's `[sample]` as [`Sampler::from_home`] reads it, with the
    /// `anthropic` provider's key read from `slot` on every call; sampling
    /// is off (`NoKey`) while the slot is empty. A `stub` provider ignores
    /// the slot.
    pub fn from_home_with_slot(home_root: &std::path::Path, slot: KeySlot) -> Sampler {
        let mut s = match HomeConfig::load(home_root).and_then(|c| c.sample()) {
            Ok(cfg) if cfg.provider == "stub" => Sampler::from_config(&cfg, |_| None),
            Ok(cfg) => {
                let p = anthropic::AnthropicProvider::with_source(
                    cfg.base_url.clone(),
                    KeySource::Slot(slot.clone()),
                );
                let mut s = Sampler::build(
                    Some(Arc::new(p)),
                    None,
                    None,
                    SampleSettings::from_config(&cfg),
                );
                s.reads_slot = true;
                s
            }
            Err(e) => Sampler::bad_config(e),
        };
        s.slot = Some(slot);
        s
    }

    /// The slot this sampler reads, when it was built with one.
    pub fn key_slot(&self) -> Option<&KeySlot> {
        self.slot.as_ref()
    }

    /// Whether the provider waits on an empty slot.
    fn slot_empty(&self) -> bool {
        self.reads_slot && self.slot.as_ref().is_some_and(|s| !s.is_set())
    }

    /// Why sampling is off; `None` when a provider is configured (and, for
    /// a slot sampler, its slot holds a key).
    pub fn reason(&self) -> Option<OffReason> {
        if self.slot_empty() {
            return Some(OffReason::NoKey);
        }
        self.reason.clone()
    }

    /// The variable the `anthropic` provider reads its key from (its name only).
    pub fn key_env(&self) -> Option<&str> {
        self.key_env.as_deref()
    }

    /// The provider calls go to; `None` while sampling is off.
    pub fn provider(&self) -> Option<&Arc<dyn SampleProvider>> {
        if self.slot_empty() {
            return None;
        }
        self.provider.as_ref()
    }

    pub fn available(&self) -> bool {
        self.provider().is_some()
    }

    pub fn provider_name(&self) -> Option<&'static str> {
        self.provider().map(|p| p.name())
    }
}

/// A page tool's answer to one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

/// Why a tool result was not delivered.
#[derive(Debug, PartialEq)]
pub enum Deliver {
    /// No such running call, or it is not waiting for that tool call.
    NotFound,
    /// The call belongs to another viewer.
    Forbidden,
}

#[derive(Default)]
struct PendingCall {
    /// The viewer cookie that started the call (memory only; never sent anywhere).
    viewer: Option<String>,
    tools: HashMap<String, oneshot::Sender<ToolOutput>>,
}

impl Sampler {
    /// Registers a running call so its tool results can be delivered.
    pub fn open_call(&self, call_id: &str, viewer: Option<String>) {
        self.pending.lock().expect("pending lock").insert(
            call_id.to_string(),
            PendingCall {
                viewer,
                tools: HashMap::new(),
            },
        );
    }

    /// Waits for the result of tool call `tool_id` of `call_id`; `None` when the call is gone.
    pub fn expect_tool(
        &self,
        call_id: &str,
        tool_id: &str,
    ) -> Option<oneshot::Receiver<ToolOutput>> {
        let mut pending = self.pending.lock().expect("pending lock");
        let call = pending.get_mut(call_id)?;
        let (tx, rx) = oneshot::channel();
        call.tools.insert(tool_id.to_string(), tx);
        Some(rx)
    }

    /// Hands `out` to the round waiting on `tool_id`, if `viewer` started the call.
    pub fn deliver(
        &self,
        call_id: &str,
        viewer: Option<&str>,
        tool_id: &str,
        out: ToolOutput,
    ) -> Result<(), Deliver> {
        let mut pending = self.pending.lock().expect("pending lock");
        let call = pending.get_mut(call_id).ok_or(Deliver::NotFound)?;
        if call.viewer.as_deref() != viewer {
            return Err(Deliver::Forbidden);
        }
        let tx = call.tools.remove(tool_id).ok_or(Deliver::NotFound)?;
        tx.send(out).map_err(|_| Deliver::NotFound)
    }

    /// Drops a call and every tool wait it had.
    pub fn forget(&self, call_id: &str) {
        self.pending.lock().expect("pending lock").remove(call_id);
    }

    /// Calls still registered (tests).
    pub fn open_calls(&self) -> usize {
        self.pending.lock().expect("pending lock").len()
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
    fn a_slot_sampler_samples_only_while_its_slot_holds_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let slot = KeySlot::new();
        let s = Sampler::from_home_with_slot(dir.path(), slot.clone());
        assert!(s.key_slot().is_some());
        assert!(!s.available());
        assert_eq!(s.reason(), Some(OffReason::NoKey));
        assert_eq!(s.provider_name(), None);
        assert_eq!(s.key_env(), None);
        slot.set(Some("sk-test".into()));
        assert!(s.available());
        assert_eq!(s.reason(), None);
        assert_eq!(s.provider_name(), Some("anthropic"));
        slot.set(Some("  ".into()));
        assert!(!s.available());
        assert_eq!(s.reason(), Some(OffReason::NoKey));
        slot.set(Some("sk-test".into()));
        slot.set(None);
        assert!(s.provider().is_none());
        assert!(
            Sampler::from_home(dir.path(), |_| None)
                .key_slot()
                .is_none()
        );
    }

    #[test]
    fn a_stub_ignores_the_slot_but_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[sample]\nprovider = \"stub\"\n",
        )
        .unwrap();
        let s = Sampler::from_home_with_slot(dir.path(), KeySlot::new());
        assert_eq!(s.provider_name(), Some("stub"));
        assert!(s.available());
        assert!(s.key_slot().is_some());
    }

    #[test]
    fn the_slot_prints_whether_it_is_set_and_never_the_key() {
        let slot = KeySlot::new();
        assert_eq!(format!("{slot:?}"), "KeySlot { set: false }");
        slot.set(Some("sk-test".into()));
        let shown = format!("{slot:?}");
        assert_eq!(shown, "KeySlot { set: true }");
        assert!(!shown.contains("sk-test"));
        assert_eq!(slot.get().as_deref(), Some("sk-test"));
        let source = KeySource::Slot(slot.clone());
        assert!(!format!("{source:?}").contains("sk-test"));
    }

    #[test]
    fn settings_follow_the_config() {
        let cfg = SampleConfig {
            daily_call_cap: Some(3),
            max_tokens: 99,
            ..SampleConfig::default()
        };
        let s = SampleSettings::from_config(&cfg);
        assert_eq!(s.daily_cost_cap, Some(500_000));
        assert_eq!(
            (s.daily_call_cap, s.max_tokens, s.max_rounds),
            (Some(3), 99, 5)
        );
        assert_eq!(s.tool_timeout, std::time::Duration::from_secs(150));
    }
}
