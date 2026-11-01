//! Model prices from tokenulator.dev, which reconciles providers' own
//! pricing pages with aggregators: [`fetch`] reads its catalog
//! (`GET` [`SITE_DATA_URL`], no key) and [`find`] one provider's listing of
//! a model in it, at its standard rates in US dollars per million tokens.
//!
//! The catalog is `{schema_version: 1, catalog: {models: [{provider_id,
//! model_id, model, pricing: {standard: {input, output, cache_read,
//! cache_write}}, source, observed_at}]}}`; a rate it does not know is
//! `null`.

use super::transport::Transport;
use futures::StreamExt;
use serde_json::Value;
use std::time::Duration;

/// Where the catalog is.
pub const SITE_DATA_URL: &str = "https://tokenulator.dev/data/site.json";
/// The catalog schema this reads.
pub const SCHEMA_VERSION: u64 = 1;
/// The most bytes of catalog read.
const MAX_BYTES: usize = 64 << 20;

/// A model's standard rates, in US dollars per million tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    /// Cache reads; `None` where the catalog has no figure.
    pub cache_read: Option<f64>,
    /// Five-minute cache writes; `None` where the catalog has no figure.
    pub cache_write: Option<f64>,
}

/// One provider's listing of a model.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// tokenulator.dev's provider ID (`google`, `mistral`, `anthropic`, …).
    pub provider_id: String,
    pub model_id: String,
    /// The model's name (`Gemini 3.5 Flash`).
    pub name: String,
    pub rates: Rates,
    /// The page the figures were read from.
    pub source: String,
    /// When they were read (RFC 3339).
    pub observed_at: String,
}

/// Why a price could not be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupError {
    /// The catalog could not be fetched or read.
    Unavailable(String),
    /// The catalog lists no such model for that provider.
    NotListed {
        provider_id: String,
        model_id: String,
    },
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::Unavailable(m) => write!(f, "tokenulator.dev could not be read: {m}"),
            LookupError::NotListed {
                provider_id,
                model_id,
            } => write!(
                f,
                "tokenulator.dev lists no price for {model_id} from {provider_id}"
            ),
        }
    }
}

impl std::error::Error for LookupError {}

/// The catalog at `url` ([`SITE_DATA_URL`] in production), each wait
/// within `timeout`.
///
/// # Errors
/// [`LookupError::Unavailable`]: no answer, a non-2xx one, a body over 64
/// MiB, not JSON, or another schema version.
pub async fn fetch(url: &str, timeout: Duration) -> Result<Value, LookupError> {
    let bad = LookupError::Unavailable;
    let late = |what: &str| bad(format!("no {what} within {} s", timeout.as_secs()));
    let res = clax_rt::time::timeout(timeout, Transport::new().send("GET", url, &[], None))
        .await
        .map_err(|_| late("answer"))?
        .map_err(bad)?;
    if !res.success() {
        return Err(bad(format!("HTTP {}", res.status)));
    }
    let mut body = res.body;
    let mut bytes = Vec::new();
    while let Some(c) = clax_rt::time::timeout(timeout, body.next())
        .await
        .map_err(|_| late("data"))?
    {
        bytes.extend_from_slice(&c.map_err(bad)?);
        if bytes.len() > MAX_BYTES {
            return Err(bad("the catalog is too large".into()));
        }
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
    if v["schema_version"].as_u64() != Some(SCHEMA_VERSION) {
        return Err(bad(format!(
            "schema_version {} is not {SCHEMA_VERSION}",
            v["schema_version"]
        )));
    }
    Ok(v)
}

/// `provider_id`'s listing of `model_id` in `site`, the IDs compared
/// ignoring case.
///
/// # Errors
/// [`LookupError::NotListed`] when there is none, or none with standard
/// input and output rates.
pub fn find(site: &Value, provider_id: &str, model_id: &str) -> Result<Listing, LookupError> {
    let same = |v: &Value, s: &str| v.as_str().is_some_and(|x| x.eq_ignore_ascii_case(s));
    let m = site["catalog"]["models"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| same(&m["provider_id"], provider_id) && same(&m["model_id"], model_id));
    let not_listed = || LookupError::NotListed {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
    };
    let m = m.ok_or_else(not_listed)?;
    let std = &m["pricing"]["standard"];
    let rate = |k: &str| std[k].as_f64().filter(|r| r.is_finite() && *r >= 0.0);
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    Ok(Listing {
        provider_id: s("provider_id"),
        model_id: s("model_id"),
        name: s("model"),
        rates: Rates {
            input: rate("input").ok_or_else(not_listed)?,
            output: rate("output").ok_or_else(not_listed)?,
            cache_read: rate("cache_read"),
            cache_write: rate("cache_write"),
        },
        source: s("source"),
        observed_at: s("observed_at"),
    })
}

/// tokenulator.dev's provider ID for a provider at `base_url` speaking
/// `api`, when the host is one it knows.
pub fn provider_for(api: clax_core::config::ModelApi, base_url: &str) -> Option<&'static str> {
    use clax_core::config::ModelApi;
    let host = crate::messages::host_of(base_url);
    match (api, host.as_str()) {
        (ModelApi::Anthropic, "api.anthropic.com") => Some("anthropic"),
        (ModelApi::Gemini, "generativelanguage.googleapis.com") => Some("google"),
        (ModelApi::OpenAi, "generativelanguage.googleapis.com") => Some("google"),
        (ModelApi::OpenAi, "api.openai.com") => Some("openai"),
        (ModelApi::OpenAi, "api.mistral.ai") => Some("mistral"),
        (ModelApi::OpenAi, "api.deepseek.com") => Some("deepseek"),
        (ModelApi::OpenAi, "api.x.ai") => Some("xai"),
        (ModelApi::OpenAi, "api.z.ai") => Some("zai"),
        (ModelApi::OpenAi, "api.groq.com") => Some("groq"),
        (ModelApi::OpenAi, "api.together.xyz") => Some("together"),
        (ModelApi::OpenAi, "api.fireworks.ai") => Some("fireworks"),
        (ModelApi::OpenAi, "openrouter.ai") => Some("openrouter"),
        _ => None,
    }
}

/// Dollars per million tokens as micro-USD per million tokens, rounded.
#[must_use]
pub fn micro_usd(dollars: f64) -> i64 {
    (dollars * 1_000_000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::config::ModelApi;
    use serde_json::json;

    fn site() -> Value {
        json!({"schema_version": 1, "catalog": {"models": [
            {"provider_id": "google", "provider": "Google", "model_id": "gemini-3.5-flash",
             "model": "Gemini 3.5 Flash", "input": 1.5, "output": 9.0,
             "pricing": {"standard": {"input": 1.5, "output": 9.0, "cache_read": 0.15,
                                      "cache_write": null, "cache_write_1h": null},
                         "batch": {"input": 0.75, "output": 4.5}},
             "source": "https://ai.google.dev/gemini-api/docs/pricing",
             "observed_at": "2026-11-01T01:27:11+00:00"},
            {"provider_id": "zai", "model_id": "glm-5.3-flash", "model": "GLM 5.3 Flash",
             "pricing": {"standard": {"input": 0.15, "output": null}}},
        ]}})
    }

    #[test]
    fn a_listing_is_found_at_its_standard_rates() {
        let l = find(&site(), "Google", "Gemini-3.5-Flash").unwrap();
        assert_eq!(l.model_id, "gemini-3.5-flash");
        assert_eq!(l.name, "Gemini 3.5 Flash");
        assert_eq!(
            l.rates,
            Rates {
                input: 1.5,
                output: 9.0,
                cache_read: Some(0.15),
                cache_write: None
            }
        );
        assert_eq!(l.source, "https://ai.google.dev/gemini-api/docs/pricing");
        assert_eq!(micro_usd(l.rates.cache_read.unwrap()), 150_000);
        assert_eq!(micro_usd(0.0625), 62_500);
    }

    #[test]
    fn a_model_unlisted_or_without_rates_is_not_listed() {
        for (p, m) in [
            ("mistral", "gemini-3.5-flash"),
            ("google", "nope"),
            ("zai", "glm-5.3-flash"),
        ] {
            assert_eq!(
                find(&site(), p, m),
                Err(LookupError::NotListed {
                    provider_id: p.into(),
                    model_id: m.into()
                })
            );
        }
        assert!(find(&json!({}), "google", "x").is_err());
    }

    #[test]
    fn known_hosts_name_their_provider() {
        let g = "https://generativelanguage.googleapis.com/v1beta";
        assert_eq!(provider_for(ModelApi::Gemini, g), Some("google"));
        assert_eq!(
            provider_for(ModelApi::OpenAi, "https://API.mistral.ai/v1"),
            Some("mistral")
        );
        assert_eq!(
            provider_for(ModelApi::Anthropic, "https://api.anthropic.com"),
            Some("anthropic")
        );
        assert_eq!(
            provider_for(ModelApi::OpenAi, "https://api.subconscious.dev/v1"),
            None
        );
        assert_eq!(
            provider_for(ModelApi::Anthropic, "https://api.mistral.ai/v1"),
            None
        );
    }

    #[tokio::test]
    async fn the_catalog_is_fetched_and_its_schema_checked() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/data/site.json", listener.local_addr().unwrap());
        let bodies = [site().to_string(), json!({"schema_version": 2}).to_string()];
        tokio::spawn(async move {
            for body in bodies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(body.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });
        let v = fetch(&url, Duration::from_secs(30)).await.unwrap();
        assert!(find(&v, "google", "gemini-3.5-flash").is_ok());
        let err = fetch(&url, Duration::from_secs(30)).await.unwrap_err();
        assert!(
            matches!(&err, LookupError::Unavailable(m) if m.contains("schema_version")),
            "{err:?}"
        );
    }
}
