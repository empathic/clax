//! Model prices from tokenulator.dev, which reconciles providers' own
//! pricing pages with aggregators: [`fetch`] reads its catalog
//! (`GET` [`SITE_DATA_URL`], no key), [`listings`] every priced listing in
//! it and [`find`] one provider's listing of a model, at their standard
//! rates in US dollars per million tokens.
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

/// The catalog at `url`, each wait within `timeout`: [`SITE_DATA_URL`] in
/// production; tests pass a local server's URL, so none reaches
/// tokenulator.dev.
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

/// `m` as a listing: `None` without finite non-negative standard input and
/// output rates.
fn listing(m: &Value) -> Option<Listing> {
    let std = &m["pricing"]["standard"];
    let rate = |k: &str| std[k].as_f64().filter(|r| r.is_finite() && *r >= 0.0);
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    Some(Listing {
        provider_id: s("provider_id"),
        model_id: s("model_id"),
        name: s("model"),
        rates: Rates {
            input: rate("input")?,
            output: rate("output")?,
            cache_read: rate("cache_read"),
            cache_write: rate("cache_write"),
        },
        source: s("source"),
        observed_at: s("observed_at"),
    })
}

/// Every listing in `site` with a provider ID, a model ID and finite
/// non-negative standard input and output rates, in catalog order: of
/// listings whose IDs are the same ignoring case, only the first, the one
/// [`find`] returns.
#[must_use]
pub fn listings(site: &Value) -> Vec<Listing> {
    let mut seen = std::collections::HashSet::new();
    site["catalog"]["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(listing)
        .filter(|l| !l.provider_id.is_empty() && !l.model_id.is_empty())
        .filter(|l| {
            seen.insert((
                l.provider_id.to_ascii_lowercase(),
                l.model_id.to_ascii_lowercase(),
            ))
        })
        .collect()
}

/// `provider_id`'s listing of `model_id` in `site`, the IDs compared
/// ignoring case.
///
/// # Errors
/// [`LookupError::NotListed`] when there is none with standard input and
/// output rates.
pub fn find(site: &Value, provider_id: &str, model_id: &str) -> Result<Listing, LookupError> {
    let same = |v: &Value, s: &str| v.as_str().is_some_and(|x| x.eq_ignore_ascii_case(s));
    site["catalog"]["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| same(&m["provider_id"], provider_id) && same(&m["model_id"], model_id))
        .find_map(listing)
        .ok_or_else(|| LookupError::NotListed {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
        })
}

/// Dollars per million tokens as micro-USD per million tokens, rounded.
#[must_use]
pub fn micro_usd(dollars: f64) -> i64 {
    (dollars * 1_000_000.0).round() as i64
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
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
    fn every_listing_with_standard_input_and_output_rates_is_listed() {
        let mut site = site();
        let models = site["catalog"]["models"].as_array_mut().unwrap();
        models.push(
            json!({"provider_id": "mistral", "model_id": "free-1", "model": "Free 1",
            "pricing": {"standard": {"input": 0, "output": 0.0}}}),
        );
        models.push(
            json!({"provider_id": "mistral", "model_id": "neg", "model": "Neg",
            "pricing": {"standard": {"input": -1.0, "output": 2.0}}}),
        );
        models.push(
            json!({"provider_id": "mistral", "model_id": "text", "model": "Text",
            "pricing": {"standard": {"input": "1.0", "output": 2.0}}}),
        );
        models.push(json!({"provider_id": "mistral", "model_id": "bare", "model": "Bare"}));
        models.push(json!({"provider_id": "mistral", "model": "No ID",
            "pricing": {"standard": {"input": 1.0, "output": 2.0}}}));
        models.push(json!({"provider_id": "", "model_id": "no-provider",
            "pricing": {"standard": {"input": 1.0, "output": 2.0}}}));
        models.push(
            json!({"provider_id": "Google", "model_id": "GEMINI-3.5-flash",
            "model": "Duplicate", "pricing": {"standard": {"input": 9.0, "output": 9.0}}}),
        );
        let all = listings(&site);
        let ids: Vec<&str> = all.iter().map(|l| l.model_id.as_str()).collect();
        assert_eq!(ids, ["gemini-3.5-flash", "free-1"]);
        assert_eq!(all[0].name, "Gemini 3.5 Flash");
        assert_eq!(all[0], find(&site, "google", "gemini-3.5-flash").unwrap());
        assert_eq!(
            all[1].rates,
            Rates {
                input: 0.0,
                output: 0.0,
                cache_read: None,
                cache_write: None
            }
        );
        assert!(listings(&json!({})).is_empty());
        assert!(listings(&json!({"catalog": {"models": {}}})).is_empty());
    }

    #[test]
    fn find_returns_the_first_listing_with_rates_of_the_same_ids() {
        let site = json!({"catalog": {"models": [
            {"provider_id": "zai", "model_id": "glm-6", "model": "No rates",
             "pricing": {"standard": {"input": null, "output": 1.0}}},
            {"provider_id": "ZAI", "model_id": "GLM-6", "model": "First",
             "pricing": {"standard": {"input": 1.0, "output": 2.0}}},
            {"provider_id": "zai", "model_id": "glm-6", "model": "Second",
             "pricing": {"standard": {"input": 3.0, "output": 4.0}}},
        ]}});
        assert_eq!(find(&site, "zai", "glm-6").unwrap().name, "First");
        let all = listings(&site);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "First");
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
