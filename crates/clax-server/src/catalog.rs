//! The price catalog's refresh: tokenulator.dev's listings
//! ([`clax_sample::tokenulator`]) fetched from [`clax_http::bot::Bot::catalog_url`]
//! and cached whole in the store ([`Store::replace_price_catalog`]), so a
//! model without cost lines is priced from it and the pricing page shows
//! each line's drift from it.
//!
//! The daemon (checking every [`CHECK_EVERY`], [`spawn_refresher`]) and
//! the hosted Worker (from its alarm) refresh it once the cache is
//! [`REFRESH_EVERY`] old by the store's clock ([`refresh_if_stale`]);
//! `POST /api/admin/pricing/catalog/refresh` and `POST
//! /api/admin/pricing/tokenulator` on demand. One refresh runs at a time,
//! and those that waited on it take its outcome ([`refresh`]). A failed
//! fetch keeps the cache as it was.

use chrono::{DateTime, Utc};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_sample::tokenulator::{self, LookupError, micro_usd};
use clax_store::Store;
use clax_store::price_catalog::CatalogListing;
use std::time::Duration;

/// How old the cache may grow before it is fetched again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(24 * 3600);
/// How often the daemon looks whether the cache is due.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// The longest one fetch of the catalog may take, its answer and its
/// whole body.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The catalog URL a daemon configured by `cfg` fetches from: its `[bot]
/// price_catalog_url` (none when that is empty, `off` or not a URL
/// [`clax_core::config::base_url_ok`] takes); with no configuration,
/// tokenulator.dev's.
#[must_use]
pub fn url_of(cfg: Option<&clax_core::config::HomeConfig>) -> Option<String> {
    let Some(cfg) = cfg else {
        return Some(clax_core::config::PRICE_CATALOG_URL.to_string());
    };
    let v = cfg
        .value("bot.price_catalog_url")
        .and_then(|v| v.into_string())
        .unwrap_or_else(|| clax_core::config::PRICE_CATALOG_URL.to_string());
    clax_core::config::catalog_url_of(&v).map(String::from)
}

/// A refresh that took: how many listings the cache holds and when it
/// was fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refreshed {
    pub listings: usize,
    pub fetched_at: DateTime<Utc>,
}

/// `l` as the store keeps it, its rates in micro-USD per million tokens.
fn cached(l: tokenulator::Listing) -> CatalogListing {
    CatalogListing {
        input_micro_usd: micro_usd(l.rates.input),
        output_micro_usd: micro_usd(l.rates.output),
        cache_read_micro_usd: l.rates.cache_read.map(micro_usd),
        cache_write_micro_usd: l.rates.cache_write.map(micro_usd),
        provider_id: l.provider_id,
        model_id: l.model_id,
        name: l.name,
        source: l.source,
        observed_at: l.observed_at,
    }
}

/// 502 `tokenulator_error`: the catalog could not be had.
fn unavailable(why: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "tokenulator_error",
        why.to_string(),
    )
}

/// Fetches the catalog from the state's URL and replaces the cache with
/// its listings, stamped with the store's time. One runs at a time: a
/// refresh that waited while another ran takes that one's outcome.
///
/// # Errors
/// 502 `tokenulator_error` when no URL is set, the catalog cannot be
/// fetched or read within [`FETCH_TIMEOUT`], or it lists no valid price
/// (the cache is kept); the store's.
pub async fn refresh(s: &AppState) -> Result<Refreshed, ApiError> {
    use std::sync::atomic::Ordering;
    let flight = s.bot.catalog_flight.clone();
    let seen = flight.done.load(Ordering::Acquire);
    let mut last = flight.last.lock().await;
    if flight.done.load(Ordering::Acquire) != seen
        && let Some(o) = last.clone()
    {
        return from_outcome(o);
    }
    let r = fetch_and_store(s).await;
    *last = Some(match &r {
        Ok(r) => Ok((r.listings, r.fetched_at)),
        Err(e) => Err((e.status, e.code, e.message.clone())),
    });
    flight.done.fetch_add(1, Ordering::AcqRel);
    r
}

/// A refresh's outcome kept in the flight, as [`refresh`] answers it.
fn from_outcome(o: clax_http::bot::CatalogOutcome) -> Result<Refreshed, ApiError> {
    o.map(|(listings, fetched_at)| Refreshed {
        listings,
        fetched_at,
    })
    .map_err(|(status, code, message)| ApiError::new(status, code, message))
}

/// [`refresh`]'s work: the fetch, then the cache replaced.
async fn fetch_and_store(s: &AppState) -> Result<Refreshed, ApiError> {
    let url = s
        .bot
        .catalog_url
        .clone()
        .ok_or_else(|| unavailable("no price catalog URL is configured"))?;
    // The request is not `Send` on the Worker's one thread.
    let site = clax_rt::sendable(async move {
        clax_rt::time::timeout(FETCH_TIMEOUT, tokenulator::fetch(&url, FETCH_TIMEOUT)).await
    })
    .await
    .map_err(|_| {
        unavailable(LookupError::Unavailable(format!(
            "the catalog was not read within {} s",
            FETCH_TIMEOUT.as_secs()
        )))
    })?
    .map_err(unavailable)?;
    let listings: Vec<CatalogListing> = tokenulator::listings(&site)
        .into_iter()
        .map(cached)
        .collect();
    let r = s
        .store_call(move |st| {
            let fetched_at = st.now_utc();
            let n = st.replace_price_catalog(&listings, fetched_at)?;
            Ok(Refreshed {
                listings: n,
                fetched_at,
            })
        })
        .await?;
    if r.listings == 0 {
        return Err(unavailable(LookupError::Unavailable(
            "the catalog lists no valid price".into(),
        )));
    }
    Ok(r)
}

/// Whether a cache fetched at `fetched_at` (`None`: empty) is due a
/// refresh at `now`: empty, or [`REFRESH_EVERY`] old or older.
#[must_use]
pub fn is_stale(fetched_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    fetched_at.is_none_or(|t| {
        now.signed_duration_since(t)
            >= chrono::Duration::from_std(REFRESH_EVERY).unwrap_or(chrono::Duration::MAX)
    })
}

/// [`refresh`] when the cache is stale by the store's clock ([`is_stale`]);
/// `None` when it is not, or no catalog URL is set.
pub async fn refresh_if_stale(s: &AppState) -> Option<Result<Refreshed, ApiError>> {
    s.bot.catalog_url.as_ref()?;
    let stale = s
        .store_call(|st: &Store| Ok(is_stale(st.catalog_fetched_at()?, st.now_utc())))
        .await
        .unwrap_or(true);
    if !stale {
        return None;
    }
    Some(refresh(s).await)
}

/// Looks now and then every `every` (the daemon passes [`CHECK_EVERY`])
/// whether the cache is due, and refreshes it when it is
/// ([`refresh_if_stale`]), until the task is aborted; a failure is logged
/// and the cache kept. Nothing runs without a catalog URL.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_refresher(s: AppState, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if s.bot.catalog_url.is_none() {
            return;
        }
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match refresh_if_stale(&s).await {
                None => {}
                Some(Ok(r)) => tracing::info!(listings = r.listings, "refreshed the price catalog"),
                Some(Err(e)) => {
                    tracing::warn!(error = %e.message, "price catalog refresh failed; kept the cache")
                }
            }
        }
    })
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_cache_is_stale_when_empty_or_a_day_old() {
        let now = t("2026-11-02T12:00:00Z");
        assert!(is_stale(None, now));
        assert!(!is_stale(Some(t("2026-11-01T12:00:01Z")), now));
        assert!(is_stale(Some(t("2026-11-01T12:00:00Z")), now));
        assert!(is_stale(Some(t("2026-10-01T00:00:00Z")), now));
        // A cache stamped ahead of the clock is fresh.
        assert!(!is_stale(Some(t("2026-11-03T00:00:00Z")), now));
    }

    #[test]
    fn the_configuration_names_the_url_or_none() {
        let cfg = |text: &str, env: &[(&str, &str)]| {
            clax_core::config::HomeConfig::from_text(
                std::path::PathBuf::from("/h/config.toml"),
                text,
                env.iter().map(|(k, v)| (k.to_string(), v.to_string())),
            )
            .unwrap()
        };
        let site = clax_core::config::PRICE_CATALOG_URL;
        assert_eq!(url_of(None).as_deref(), Some(site));
        assert_eq!(url_of(Some(&cfg("", &[]))).as_deref(), Some(site));
        assert_eq!(
            url_of(Some(&cfg("", &[("CLAX_BOT__PRICE_CATALOG_URL", "off")]))),
            None
        );
        assert_eq!(
            url_of(Some(&cfg("[bot]\nprice_catalog_url = \"\"\n", &[]))),
            None
        );
        assert_eq!(
            url_of(Some(&cfg(
                "[bot]\nprice_catalog_url = \"http://127.0.0.1:9/site.json\"\n",
                &[]
            )))
            .as_deref(),
            Some("http://127.0.0.1:9/site.json")
        );
        // Not a URL the key may name: none, as an invalid [bot] turns
        // ClaxBot off.
        assert_eq!(
            url_of(Some(&cfg(
                "[bot]\nprice_catalog_url = \"http://evil.example/\"\n",
                &[]
            ))),
            None
        );
    }
}
