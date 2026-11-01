//! The cached price catalog (migration 60): tokenulator.dev's listings of
//! what each provider charges for each model, kept so a model with no cost
//! line is priced from it ([`crate::credits::RateSource::Catalog`]) and an
//! admin sees where a line's cost drifted from it
//! ([`Store::model_pricing`]). The catalog is fetched outside the store
//! (the daemon, the Worker, the admin pages) and replaced here whole.
//!
//! Prices are integer micro-USD per million tokens. A model ID is matched
//! ignoring ASCII case, an exact match first.

use super::Store;
use super::credits::MAX_COST_MICRO_USD;
use chrono::{DateTime, Utc};
use clax_core::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

/// One model's listing in the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogListing {
    /// The catalog's provider ID (`anthropic`, `openai`, `openrouter`).
    pub provider_id: String,
    /// The model ID as the provider names it.
    pub model_id: String,
    pub name: String,
    pub input_micro_usd: i64,
    pub output_micro_usd: i64,
    /// Cache reads; `None` when the catalog lists none.
    pub cache_read_micro_usd: Option<i64>,
    /// Cache writes; `None` when the catalog lists none.
    pub cache_write_micro_usd: Option<i64>,
    /// The page the prices were read from.
    pub source: String,
    /// When the catalog observed the prices, as it says.
    pub observed_at: String,
}

impl CatalogListing {
    /// Whether the listing is one to keep: IDs of 1 to 200 bytes, a name of
    /// 1 to 200 characters, a source of at most 500 and an observation time
    /// of at most 64, and every price from 0 to [`MAX_COST_MICRO_USD`].
    fn valid(&self) -> bool {
        let id = |s: &str| !s.is_empty() && s.len() <= 200;
        let chars = |s: &str| s.chars().count();
        let price = |v: i64| (0..=MAX_COST_MICRO_USD).contains(&v);
        id(&self.provider_id)
            && id(&self.model_id)
            && (1..=200).contains(&chars(&self.name))
            && chars(&self.source) <= 500
            && chars(&self.observed_at) <= 64
            && price(self.input_micro_usd)
            && price(self.output_micro_usd)
            && self.cache_read_micro_usd.is_none_or(price)
            && self.cache_write_micro_usd.is_none_or(price)
    }
}

const COLS: &str = "provider_id, model_id, name, input_micro_usd, output_micro_usd,
    cache_read_micro_usd, cache_write_micro_usd, source, observed_at";

fn row_to_listing(r: &Row<'_>) -> rusqlite::Result<CatalogListing> {
    Ok(CatalogListing {
        provider_id: r.get(0)?,
        model_id: r.get(1)?,
        name: r.get(2)?,
        input_micro_usd: r.get(3)?,
        output_micro_usd: r.get(4)?,
        cache_read_micro_usd: r.get(5)?,
        cache_write_micro_usd: r.get(6)?,
        source: r.get(7)?,
        observed_at: r.get(8)?,
    })
}

/// `provider`'s listing of `model` on `c`, the model ID compared ignoring
/// ASCII case (an exact match first), or `None`.
pub(crate) fn listing_in(
    c: &Connection,
    provider: &str,
    model: &str,
) -> Result<Option<CatalogListing>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {COLS} FROM price_catalog
             WHERE provider_id = ?1 AND model_id = ?2 COLLATE NOCASE
             ORDER BY model_id = ?2 DESC, model_id LIMIT 1"
    ))?
    .query_row(params![provider, model], row_to_listing)
    .optional()?)
}

impl Store {
    /// Replaces the whole catalog with `listings`, fetched at `fetched_at`,
    /// in one transaction, and returns how many rows it holds now. Listings
    /// that are not valid (an empty or overlong ID or name, an overlong
    /// source or observation time, a price below 0 or above
    /// [`MAX_COST_MICRO_USD`]) are left out, and of two with the same
    /// provider and model the later is kept. With nothing to keep, the
    /// catalog is left as it was (a failed or empty fetch never empties
    /// it) and 0 is returned.
    ///
    /// # Errors
    /// The store's.
    pub fn replace_price_catalog(
        &self,
        listings: &[CatalogListing],
        fetched_at: DateTime<Utc>,
    ) -> Result<usize> {
        let keep: Vec<&CatalogListing> = listings.iter().filter(|l| l.valid()).collect();
        if keep.is_empty() {
            return Ok(0);
        }
        let stamp = clax_core::time::stamp(fetched_at);
        self.with_tx(|tx| {
            tx.execute("DELETE FROM price_catalog", [])?;
            let mut insert = tx.prepare_cached(&format!(
                "INSERT OR REPLACE INTO price_catalog ({COLS}, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
            ))?;
            for l in &keep {
                insert.execute(params![
                    l.provider_id,
                    l.model_id,
                    l.name,
                    l.input_micro_usd,
                    l.output_micro_usd,
                    l.cache_read_micro_usd,
                    l.cache_write_micro_usd,
                    l.source,
                    l.observed_at,
                    stamp,
                ])?;
            }
            Ok(tx.query_row("SELECT count(*) FROM price_catalog", [], |r| {
                r.get::<_, i64>(0)
            })? as usize)
        })
    }

    /// `provider`'s listing of `model` in the catalog, the model ID compared
    /// ignoring ASCII case, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn catalog_listing(&self, provider: &str, model: &str) -> Result<Option<CatalogListing>> {
        self.with_read(|c| listing_in(c, provider, model))
    }

    /// When the catalog was last replaced ([`Store::replace_price_catalog`]),
    /// or `None` while it is empty.
    ///
    /// # Errors
    /// The store's.
    pub fn catalog_fetched_at(&self) -> Result<Option<DateTime<Utc>>> {
        let at: Option<String> = self.with_read(|c| {
            Ok(
                c.query_row("SELECT max(fetched_at) FROM price_catalog", [], |r| {
                    r.get(0)
                })?,
            )
        })?;
        Ok(at
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| t.with_timezone(&Utc)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_util::store;

    /// A listing of `model` by `provider` at `input` and `output`, without
    /// cache rates.
    pub(crate) fn listing(provider: &str, model: &str, input: i64, output: i64) -> CatalogListing {
        CatalogListing {
            provider_id: provider.into(),
            model_id: model.into(),
            name: model.into(),
            input_micro_usd: input,
            output_micro_usd: output,
            cache_read_micro_usd: None,
            cache_write_micro_usd: None,
            source: "https://tokenulator.dev/".into(),
            observed_at: "2026-10-30".into(),
        }
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_catalog_is_replaced_whole_and_never_emptied() {
        let (_d, st) = store();
        assert_eq!(st.catalog_fetched_at().unwrap(), None);
        let n = st
            .replace_price_catalog(
                &[
                    listing("openai", "gpt-5", 1_250_000, 10_000_000),
                    listing("openai", "gpt-5-mini", 250_000, 2_000_000),
                ],
                t("2026-10-30T12:00:00Z"),
            )
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            st.catalog_fetched_at().unwrap(),
            Some(t("2026-10-30T12:00:00Z"))
        );
        // A new fetch replaces every row: gpt-5-mini is gone.
        let n = st
            .replace_price_catalog(
                &[
                    listing("openai", "gpt-5", 1_000_000, 8_000_000),
                    // Not valid: left out.
                    listing("openai", "bad", -1, 1),
                    listing("", "nameless", 1, 1),
                ],
                t("2026-10-31T12:00:00Z"),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(st.catalog_listing("openai", "gpt-5-mini").unwrap(), None);
        assert_eq!(
            st.catalog_listing("openai", "gpt-5")
                .unwrap()
                .unwrap()
                .input_micro_usd,
            1_000_000
        );
        // Nothing to keep leaves the catalog as it was.
        assert_eq!(
            st.replace_price_catalog(&[], t("2026-11-01T12:00:00Z"))
                .unwrap(),
            0
        );
        assert_eq!(
            st.replace_price_catalog(&[listing("x", "y", -5, 0)], t("2026-11-01T12:00:00Z"))
                .unwrap(),
            0
        );
        assert!(st.catalog_listing("openai", "gpt-5").unwrap().is_some());
        assert_eq!(
            st.catalog_fetched_at().unwrap(),
            Some(t("2026-10-31T12:00:00Z"))
        );
    }

    #[test]
    fn of_two_listings_of_one_model_the_later_is_kept() {
        let (_d, st) = store();
        let n = st
            .replace_price_catalog(
                &[
                    listing("openai", "gpt-5", 1, 2),
                    listing("openai", "gpt-5", 3, 4),
                ],
                t("2026-10-30T12:00:00Z"),
            )
            .unwrap();
        assert_eq!(n, 1);
        let l = st.catalog_listing("openai", "gpt-5").unwrap().unwrap();
        assert_eq!((l.input_micro_usd, l.output_micro_usd), (3, 4));
    }

    #[test]
    fn a_listing_with_an_unbounded_field_is_left_out() {
        let (_d, st) = store();
        let with = |f: fn(&mut CatalogListing)| {
            let mut l = listing("openai", "gpt-5", 1, 2);
            f(&mut l);
            l
        };
        let bad = [
            with(|l| l.name = String::new()),
            with(|l| l.name = "n".repeat(201)),
            with(|l| l.source = "s".repeat(501)),
            with(|l| l.observed_at = "o".repeat(65)),
            with(|l| l.model_id = "m".repeat(201)),
            with(|l| l.cache_read_micro_usd = Some(-1)),
            with(|l| l.cache_write_micro_usd = Some(MAX_COST_MICRO_USD + 1)),
        ];
        for l in &bad {
            assert!(!l.valid(), "{l:?}");
        }
        // 200 characters of a name in more bytes is within bounds; an ID is
        // bounded in bytes.
        assert!(with(|l| l.name = "é".repeat(200)).valid());
        assert!(!with(|l| l.model_id = "é".repeat(101)).valid());
        assert_eq!(
            st.replace_price_catalog(&bad, t("2026-10-30T12:00:00Z"))
                .unwrap(),
            0
        );
        assert_eq!(st.catalog_fetched_at().unwrap(), None);
    }

    #[test]
    fn a_listing_is_found_ignoring_case_an_exact_match_first() {
        let (_d, st) = store();
        st.replace_price_catalog(
            &[
                listing("openrouter", "Qwen/Qwen3-Coder", 200_000, 800_000),
                listing("openrouter", "qwen/qwen3-coder", 300_000, 900_000),
                listing("mistral", "mistral-large", 2_000_000, 6_000_000),
            ],
            t("2026-10-30T12:00:00Z"),
        )
        .unwrap();
        let get = |p: &str, m: &str| {
            st.catalog_listing(p, m)
                .unwrap()
                .map(|l| (l.model_id, l.input_micro_usd))
        };
        assert_eq!(
            get("openrouter", "qwen/qwen3-coder"),
            Some(("qwen/qwen3-coder".into(), 300_000))
        );
        assert_eq!(
            get("openrouter", "Qwen/Qwen3-Coder"),
            Some(("Qwen/Qwen3-Coder".into(), 200_000))
        );
        assert_eq!(
            get("mistral", "MISTRAL-LARGE"),
            Some(("mistral-large".into(), 2_000_000))
        );
        // Another provider's listing is not this one's.
        assert_eq!(get("openai", "mistral-large"), None);
    }
}
