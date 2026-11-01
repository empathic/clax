//! The price catalog's refresh: tokenulator.dev's listings
//! ([`clax_sample::tokenulator`]) fetched from [`clax_http::bot::Bot::catalog_url`]
//! and cached whole in the store ([`Store::replace_price_catalog`]), so a
//! model without cost lines is priced from it and the pricing page shows
//! each line's drift from it.
//!
//! The daemon refreshes it at start and every [`REFRESH_EVERY`]
//! ([`spawn_refresher`]); the hosted Worker from its alarm once the cache is
//! older than that ([`refresh_if_stale`]); `POST /api/admin/pricing/catalog/refresh`
//! and `POST /api/admin/pricing/tokenulator` on demand. A failed fetch
//! keeps the cache as it was.

use chrono::{DateTime, Utc};
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_sample::tokenulator::{self, LookupError, micro_usd};
use clax_store::Store;
use clax_store::price_catalog::CatalogListing;
use std::time::Duration;

/// How old the cache may grow before it is fetched again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(24 * 3600);
/// How long each wait for the catalog may last.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// The variable that names the daemon's catalog URL: unset, tokenulator.dev's
/// ([`tokenulator::SITE_DATA_URL`]); empty, none (the catalog is never
/// fetched); else that URL.
pub const URL_ENV: &str = "CLAX_PRICE_CATALOG_URL";

/// The catalog URL `var` (the value of [`URL_ENV`]) names.
#[must_use]
pub fn url_from(var: Option<&str>) -> Option<String> {
    match var.map(str::trim) {
        None => Some(tokenulator::SITE_DATA_URL.to_string()),
        Some("") => None,
        Some(u) => Some(u.to_string()),
    }
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
/// its listings, stamped with the store's time.
///
/// # Errors
/// 502 `tokenulator_error` when no URL is set, the catalog cannot be
/// fetched or read, or it lists no valid price (the cache is kept); the
/// store's.
pub async fn refresh(s: &AppState) -> Result<Refreshed, ApiError> {
    let url = s
        .bot
        .catalog_url
        .clone()
        .ok_or_else(|| unavailable("no price catalog URL is configured"))?;
    // The request is not `Send` on the Worker's one thread.
    let site = clax_rt::sendable(async move { tokenulator::fetch(&url, FETCH_TIMEOUT).await })
        .await
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

/// Refreshes the catalog now and then every `every` (the daemon passes
/// [`REFRESH_EVERY`]), until the task is aborted; a failure is logged and
/// the cache kept. Nothing runs without a catalog URL.
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
            match refresh(&s).await {
                Ok(r) => tracing::info!(listings = r.listings, "refreshed the price catalog"),
                Err(e) => {
                    tracing::warn!(error = %e.message, "price catalog refresh failed; kept the cache")
                }
            }
        }
    })
}

#[cfg(test)]
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
    fn the_variable_names_the_url_or_none() {
        assert_eq!(url_from(None).as_deref(), Some(tokenulator::SITE_DATA_URL));
        assert_eq!(url_from(Some("")), None);
        assert_eq!(url_from(Some("  ")), None);
        assert_eq!(
            url_from(Some("http://127.0.0.1:9/site.json")).as_deref(),
            Some("http://127.0.0.1:9/site.json")
        );
    }
}
