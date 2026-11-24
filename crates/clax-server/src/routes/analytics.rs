//! Site analytics' setting and read on a hosted Clax
//! (`clax_store::analytics`).
//!
//! - The setting, `analytics` (`on`, every artifact's at first, or `off`):
//!   carried by `GET /api/artifacts/<id>` ([`field`]) for a caller who
//!   administers the artifact on a hosted Clax, and changed by `PATCH
//!   /api/artifacts/<id> {"analytics": "on" | "off"}`
//!   ([`super::artifacts::patch`], [`super::setting_change`]), which never
//!   needs approval. Turning it off stops counting and keeps what was
//!   counted.
//! - `GET /api/artifacts/<id>/analytics?from=YYYY-MM-DD&to=YYYY-MM-DD`
//!   ([`read`]): the artifact's totals over a range of UTC days, for the
//!   shell's Analytics panel.
//!
//! The read serves the people who administer the page, never an agent or
//! the page itself: 404 `not_found` on a local Clax, 403 `not_for_agents`
//! for the owner's agents and `forbidden` for a person's (`Clax-Grant`),
//! 403 `not_for_pages` for a request with the page marker (`x-clax-via:
//! page`), 403 `admin_only` for a caller who does not administer it
//! ([`clax_http::agent::administers`]), 404 for an unknown artifact.

use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use chrono::{Days, NaiveDate};
use clax_core::analytics::{Dim, DimRow, Metric, oldest_kept, parse_day};
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::{DeniedToGrants, administers, page_marker};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::state::AppState;
use clax_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

/// How many days the read's range spans when `from` is left out: the 30
/// days ending `to`.
pub const DEFAULT_DAYS: u64 = 30;
/// The most days a range spans.
pub const MAX_DAYS: u64 = 400;
/// The rows each dimension's table lists before the rest are summed into
/// one `(other)` row.
pub const TOP: usize = 50;

/// Parses `analytics`: `on` (true) or `off`; 400 `invalid_argument` for
/// anything else.
///
/// # Errors
/// As above.
pub fn parse(v: &str) -> Result<bool, ApiError> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(ApiError::bad_request(
            "invalid_argument",
            format!("analytics is on or off, not '{v}'"),
        )),
    }
}

/// 400 `invalid_argument`: a local Clax counts nothing, so takes no
/// `analytics`.
pub fn local() -> ApiError {
    ApiError::bad_request("invalid_argument", "a local Clax counts no visits")
}

/// Artifact `id`'s `analytics` as `GET /api/artifacts/<id>` carries it for
/// `caller`: `on` or `off` on a hosted Clax (`hosted`) for a caller who
/// administers it, else `None` (left out), as for a draft, which has no
/// setting of its own.
///
/// # Errors
/// The store's.
pub fn field(
    st: &Store,
    caller: &Caller,
    id: &ArtifactId,
    hosted: bool,
) -> clax_core::Result<Option<&'static str>> {
    if !hosted || st.is_hidden(id.as_str()) || !administers(st, caller, id.as_str(), true)? {
        return Ok(None);
    }
    Ok(Some(if st.analytics_on(id)? { "on" } else { "off" }))
}

/// The read's query: the range's first and last days, both included.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Range {
    from: Option<String>,
    to: Option<String>,
}

/// The range `q` names on day `today`: `to` (default `today`) and `from`
/// (default [`DEFAULT_DAYS`] days ending `to`), each `YYYY-MM-DD`, `from`
/// not after `to`, at most [`MAX_DAYS`] days, and none before the oldest
/// day kept ([`oldest_kept`]).
///
/// # Errors
/// 400 `invalid_argument` otherwise.
pub fn range(q: &Range, today: NaiveDate) -> Result<(NaiveDate, NaiveDate), ApiError> {
    let bad = |m: String| ApiError::bad_request("invalid_argument", m);
    let day = |name: &str, v: &str| {
        parse_day(v).ok_or_else(|| bad(format!("{name} is a day, YYYY-MM-DD, not '{v}'")))
    };
    let to =
        q.to.as_deref()
            .map(|v| day("to", v))
            .transpose()?
            .unwrap_or(today);
    let from = match q.from.as_deref() {
        Some(v) => day("from", v)?,
        None => to
            .checked_sub_days(Days::new(DEFAULT_DAYS - 1))
            .unwrap_or(NaiveDate::MIN),
    };
    if from > to {
        return Err(bad("from is after to".into()));
    }
    if (to - from).num_days() + 1 > i64::try_from(MAX_DAYS).unwrap_or(i64::MAX) {
        return Err(bad(format!("a range spans at most {MAX_DAYS} days")));
    }
    let oldest = oldest_kept(today);
    if from < oldest {
        return Err(bad(format!(
            "totals are kept from {} on",
            oldest.format("%Y-%m-%d")
        )));
    }
    Ok((from, to))
}

fn rows(rows: Vec<DimRow>) -> Value {
    Value::Array(
        rows.into_iter()
            .map(|r| json!({"value": r.value, "views": r.counts.views, "visitors": r.counts.visitors}))
            .collect(),
    )
}

/// `GET /api/artifacts/<id>/analytics?from=YYYY-MM-DD&to=YYYY-MM-DD`
/// (UTC days, both included; the module docs say who may read it): `{
/// analytics, from, to, totals: {visitors, views, framed, load_ms_median,
/// lcp_ms_median}, days: [{day, visitors, views, framed}], versions: [{n,
/// published_at}], pages, referrers, countries, devices, versions_viewed }`.
///
/// - `days` holds every day of the range, oldest first, zeros included;
///   `totals` sums them (a visitor on two days counts twice); a median is
///   null without samples.
/// - `versions` are those published in the range, oldest first.
/// - Each table (`[{value, views, visitors}]`) lists its top [`TOP`] values
///   by views, the rest summed into one `(other)` row.
/// - Views not yet written are merged in.
///
/// `Cache-Control: private, no-store`.
///
/// # Errors
/// The module docs' refusals, and 400 `invalid_argument` for a range
/// [`range`] refuses.
pub async fn read(
    State(s): State<AppState>,
    d: DeniedToGrants,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<Range>, QueryRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    if s.hosted.is_none() {
        return Err(ApiError::not_found());
    }
    let id = parse_id(&aid.map(|Path(a)| a).map_err(|_| ApiError::not_found())?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let caller = d.caller(&s).await?;
    if caller.is_agent() {
        return Err(clax_http::auth::not_for_agents());
    }
    if page_marker(&caller, &headers)? {
        return Err(ApiError::forbidden(
            "not_for_pages",
            "a page's analytics are read by those who administer it in Clax, never by the page",
        ));
    }
    let (c, i2) = (caller.clone(), id.clone());
    let (admin, today) = s
        .store_call(move |st| {
            st.get_artifact(&i2)?.ok_or(CoreError::NotFound)?;
            Ok((
                administers(st, &c, i2.as_str(), true)?,
                st.now_utc().date_naive(),
            ))
        })
        .await?;
    if !admin {
        return Err(ApiError::forbidden(
            "admin_only",
            "only those who administer the page read its analytics",
        ));
    }
    let (from, to) = range(&q, today)?;
    let i2 = id.clone();
    let (on, mut t) = s
        .store_call(move |st| Ok((st.analytics_on(&i2)?, st.analytics_range(&i2, from, to)?)))
        .await?;
    // Views counted but not yet written, so a view shows up at once.
    for (day, sums) in s.analytics.pending(id.as_str()) {
        t.merge_day(&day, &sums);
    }
    let totals = t.totals();
    let v = json!({
        "analytics": if on { "on" } else { "off" },
        "from": from.format("%Y-%m-%d").to_string(),
        "to": to.format("%Y-%m-%d").to_string(),
        "totals": {
            "visitors": totals.visitors,
            "views": totals.views,
            "framed": totals.framed,
            "load_ms_median": t.median(Metric::Load),
            "lcp_ms_median": t.median(Metric::Lcp),
        },
        "days": t.days.iter().map(|d| json!({
            "day": d.day, "visitors": d.visitors, "views": d.views, "framed": d.framed,
        })).collect::<Vec<_>>(),
        "versions": t.versions.iter().map(|m| json!({"n": m.n, "published_at": m.published_at})).collect::<Vec<_>>(),
        "pages": rows(t.top(Dim::Path, TOP)),
        "referrers": rows(t.top(Dim::Referrer, TOP)),
        "countries": rows(t.top(Dim::Country, TOP)),
        "devices": rows(t.top(Dim::Device, TOP)),
        "versions_viewed": rows(t.top(Dim::Version, TOP)),
    });
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(v)).into_response())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        parse_day(s).unwrap()
    }

    fn q(from: Option<&str>, to: Option<&str>) -> Range {
        Range {
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        }
    }

    #[test]
    fn a_range_defaults_to_the_thirty_days_ending_today() {
        let today = day("2026-11-19");
        assert_eq!(
            range(&q(None, None), today).unwrap(),
            (day("2026-10-21"), today)
        );
        assert_eq!(
            range(&q(None, Some("2026-11-01")), today).unwrap(),
            (day("2026-10-03"), day("2026-11-01"))
        );
        assert_eq!(
            range(&q(Some("2026-11-19"), None), today).unwrap(),
            (today, today)
        );
    }

    #[test]
    fn a_range_is_ordered_within_four_hundred_days_and_retention() {
        let today = day("2026-11-19");
        let refused = |from: &str, to: &str| range(&q(Some(from), Some(to)), today).is_err();
        assert!(refused("2026-11-02", "2026-11-01"));
        assert!(refused("2026-11-1", "2026-11-19"));
        assert!(refused("2026-11-01", "tomorrow"));
        // Thirteen calendar months back is kept; the day before is not.
        assert!(!refused("2025-10-19", "2026-11-19"));
        assert!(refused("2025-10-18", "2026-11-19"));
        // 400 days at most.
        assert!(!refused("2026-11-19", "2027-12-23"));
        assert!(refused("2026-11-19", "2027-12-24"));
    }

    #[test]
    fn analytics_is_on_or_off() {
        assert!(parse("on").unwrap());
        assert!(!parse("off").unwrap());
        assert!(parse("ON").is_err());
        assert!(parse("").is_err());
    }
}
