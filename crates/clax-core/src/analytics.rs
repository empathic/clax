//! Site analytics' rules that need no database or request: the dimensions
//! a page view is counted under, how a request's User-Agent, client hints,
//! country header and reported referrer become their values, which
//! User-Agents are not counted, the load timing histogram's buckets, UTC
//! day keys and retention, and how one view adds to a day's sums.
//!
//! The store keeps the sums (`clax_store::analytics`); the server reads the
//! request and keeps the batch not yet written (`clax_http::analytics`).

use chrono::{DateTime, Days, Months, NaiveDate, Utc};
use std::collections::BTreeMap;

/// What a page view is counted under in `analytics_dims.dim`. The column
/// has no SQL `CHECK`: the set is this enum's, so a later dimension (a
/// page's own events) is new rows, not a rebuilt table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dim {
    /// The published path of the page viewed.
    Path,
    /// The referring site's host, `""` for none or unknown.
    Referrer,
    /// The viewer's country: two upper-case characters, `XX` unknown.
    Country,
    /// The viewer's device class ([`Device`]).
    Device,
    /// The version viewed, in decimal.
    Version,
}

impl Dim {
    /// Every dimension, in the order the read API lists them.
    pub const ALL: [Dim; 5] = [
        Dim::Path,
        Dim::Referrer,
        Dim::Country,
        Dim::Device,
        Dim::Version,
    ];

    /// Its value in `analytics_dims.dim`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Dim::Path => "path",
            Dim::Referrer => "referrer",
            Dim::Country => "country",
            Dim::Device => "device",
            Dim::Version => "version",
        }
    }

    /// The dimension named `s` ([`Dim::as_str`]), or `None`.
    pub fn parse(s: &str) -> Option<Dim> {
        Dim::ALL.into_iter().find(|d| d.as_str() == s)
    }

    /// Whether an artifact's day takes at most [`VALUE_CAP`] distinct values
    /// of this dimension, the rest counted as [`OTHER`]: the dimensions a
    /// page chooses the values of. Countries, devices and versions are
    /// bounded by what they are.
    pub const fn capped(self) -> bool {
        matches!(self, Dim::Path | Dim::Referrer)
    }
}

/// The most distinct values of a [`Dim::capped`] dimension an artifact's
/// day takes; a value past it is counted as [`OTHER`].
pub const VALUE_CAP: usize = 500;

/// The value standing for every value past a cap, in the store and in the
/// read API's tables.
pub const OTHER: &str = "(other)";

/// A viewer's device class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Device {
    Phone,
    Tablet,
    Desktop,
}

impl Device {
    /// Its value under [`Dim::Device`].
    pub const fn as_str(self) -> &'static str {
        match self {
            Device::Phone => "phone",
            Device::Tablet => "tablet",
            Device::Desktop => "desktop",
        }
    }
}

/// The device class of a request with the User-Agent `ua` and the
/// `Sec-CH-UA-Mobile` header `ch_mobile`: a phone when the hint is `?1`;
/// else by the User-Agent, a tablet for `iPad`, `Tablet`, or `Android`
/// without `Mobile`, a phone for `Mobi`, `iPhone` or `Android`, and a
/// desktop otherwise. iPadOS Safari sends a Mac's User-Agent and is a
/// desktop.
pub fn device(ua: &str, ch_mobile: Option<&str>) -> Device {
    if ch_mobile.map(str::trim) == Some("?1") {
        return Device::Phone;
    }
    let android = ua.contains("Android");
    if ua.contains("iPad") || ua.contains("Tablet") || (android && !ua.contains("Mobile")) {
        Device::Tablet
    } else if ua.contains("Mobi") || ua.contains("iPhone") || android {
        Device::Phone
    } else {
        Device::Desktop
    }
}

/// Words, matched without regard to case, whose presence in a User-Agent
/// names a bot, a crawler, a spider, a link preview fetcher or a headless
/// browser.
const NOT_A_VIEWER: [&str; 12] = [
    "bot",
    "crawl",
    "spider",
    "slurp",
    "preview",
    "headless",
    "phantomjs",
    "facebookexternalhit",
    "linkexpanding",
    "embedly",
    "whatsapp",
    "lighthouse",
];

/// Whether the User-Agent `ua` is not a person's browser: empty, or naming
/// one of a short fixed list of bots, crawlers, spiders, preview fetchers
/// and headless browsers. A view from one is not counted.
pub fn is_bot(ua: &str) -> bool {
    let ua = ua.trim().to_ascii_lowercase();
    ua.is_empty() || NOT_A_VIEWER.iter().any(|w| ua.contains(w))
}

/// The country under [`Dim::Country`] for the `CF-IPCountry` header
/// `header` (when trusted): its two letters in upper case, or `XX` for a
/// missing or malformed one. Cloudflare's `T1` (Tor) is kept as given.
pub fn country(header: Option<&str>) -> String {
    match header.map(str::trim).map(str::as_bytes) {
        Some([a, b]) if a.is_ascii_alphabetic() && b.is_ascii_alphanumeric() => {
            format!(
                "{}{}",
                a.to_ascii_uppercase() as char,
                b.to_ascii_uppercase() as char
            )
        }
        _ => "XX".to_string(),
    }
}

/// The longest host name a referrer may be, in bytes.
pub const REFERRER_MAX: usize = 253;

/// The referring host under [`Dim::Referrer`] for a report's `referrer`:
/// the host name in lower case when it is at most [`REFERRER_MAX`] bytes
/// of letters, digits, `-` and `.` and neither an IP address nor a host of
/// this Clax, else `""` ("no referring site"). An IP address is never kept:
/// an IPv6 literal fails the characters (its brackets and colons), and a
/// host whose last label is all digits is an IPv4 address in one of its
/// forms (no top-level domain is numeric). A host of this Clax is one of `own` (the site's
/// own host and the public origin's host, without ports) or a host under
/// `site_domain` (any artifact's site).
pub fn referrer(raw: &str, own: &[&str], site_domain: Option<&str>) -> String {
    let host = raw.trim().to_ascii_lowercase();
    let well_formed = !host.is_empty()
        && host.len() <= REFERRER_MAX
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && !host
            .rsplit('.')
            .next()
            .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
    if !well_formed {
        return String::new();
    }
    let ours = own.iter().any(|o| o.eq_ignore_ascii_case(&host))
        || site_domain.is_some_and(|d| {
            let d = d.trim_start_matches('.').to_ascii_lowercase();
            !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
        });
    if ours { String::new() } else { host }
}

/// Whether a version's file at `path`, stored as `content_type`, is a page
/// whose views are counted: an HTML file (`text/html`, parameters ignored),
/// or `index.html`, which is always served as a page.
pub fn is_page(path: &str, content_type: &str) -> bool {
    path == "index.html"
        || content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("text/html")
}

/// A load timing measured for a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    /// The navigation's `loadEventStart`.
    Load,
    /// The largest contentful paint seen by the load.
    Lcp,
}

impl Metric {
    /// Both metrics.
    pub const ALL: [Metric; 2] = [Metric::Load, Metric::Lcp];

    /// Its value in `analytics_timings.metric`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Metric::Load => "load",
            Metric::Lcp => "lcp",
        }
    }

    /// The metric named `s` ([`Metric::as_str`]), or `None`.
    pub fn parse(s: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// The highest timing bucket: 2^20 ms (about 17 minutes) and above.
pub const MAX_BUCKET: u8 = 80;

/// The histogram bucket of a timing of `ms` milliseconds: `floor(4 ·
/// log2(ms))` clamped to `0..=`[`MAX_BUCKET`] (quarter octaves, about 19 %
/// wide), computed on integers so a power of two falls in its own bucket.
pub fn bucket(ms: u32) -> u8 {
    if ms <= 1 {
        return 0;
    }
    // Every ms of 2^20 and above is in the last bucket; below it ms^4 fits
    // a u128, and floor(4 · log2(ms)) = floor(log2(ms^4)).
    let ms = u128::from(ms.min(1 << 20));
    let fourth = ms * ms * ms * ms;
    let b = 127 - fourth.leading_zeros();
    u8::try_from(b).map_or(MAX_BUCKET, |b| b.min(MAX_BUCKET))
}

/// The timing a bucket stands for: the geometric middle of the times it
/// holds, `2^((b + 0.5) / 4)` ms, rounded; within about 10 % of any of them.
pub fn bucket_middle(b: u8) -> u32 {
    let b = b.min(MAX_BUCKET);
    // Positive and at most 2^20.125: the cast neither wraps nor truncates.
    ((f64::from(b) + 0.5) * 0.25).exp2().round() as u32
}

/// A timing histogram: how many views fell in each [`bucket`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    counts: [u64; MAX_BUCKET as usize + 1],
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram {
            counts: [0; MAX_BUCKET as usize + 1],
        }
    }
}

impl Histogram {
    /// Adds `n` views to bucket `b` (clamped to [`MAX_BUCKET`]).
    pub fn add(&mut self, b: u8, n: u64) {
        let i = usize::from(b.min(MAX_BUCKET));
        self.counts[i] = self.counts[i].saturating_add(n);
    }

    /// Adds every view of `other`.
    pub fn merge(&mut self, other: &Histogram) {
        for (b, n) in other.buckets() {
            self.add(b, n);
        }
    }

    /// The buckets holding a view, with their counts, lowest first.
    pub fn buckets(&self) -> impl Iterator<Item = (u8, u64)> + '_ {
        (0..=MAX_BUCKET)
            .zip(self.counts.iter().copied())
            .filter(|&(_, n)| n > 0)
    }

    /// How many views it holds.
    pub fn total(&self) -> u64 {
        self.counts.iter().fold(0, |a, &n| a.saturating_add(n))
    }

    /// The median timing, as the [`bucket_middle`] of the bucket holding the
    /// lower middle view; `None` without views.
    pub fn median(&self) -> Option<u32> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        let rank = total.div_ceil(2);
        let mut seen = 0u64;
        self.buckets().find_map(|(b, n)| {
            seen += n;
            (seen >= rank).then(|| bucket_middle(b))
        })
    }
}

/// The UTC day of `t`, as the analytics tables key days: `YYYY-MM-DD`.
pub fn day_key(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

/// The day `s` names in [`day_key`]'s form, or `None`.
pub fn parse_day(s: &str) -> Option<NaiveDate> {
    if s.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The first instant of the UTC day after `t`'s: when `t`'s day rolls over.
pub fn next_midnight(t: DateTime<Utc>) -> DateTime<Utc> {
    let next = t.date_naive() + Days::new(1);
    next.and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc()
}

/// How many calendar months of daily totals are kept.
pub const RETENTION_MONTHS: u32 = 13;

/// The oldest day kept on day `today`: [`RETENTION_MONTHS`] calendar months
/// before it, on the same day of the month or, where that month is shorter,
/// its last day. Totals of earlier days are deleted.
pub fn oldest_kept(today: NaiveDate) -> NaiveDate {
    today
        .checked_sub_months(Months::new(RETENTION_MONTHS))
        .unwrap_or(NaiveDate::MIN)
}

/// A page view, as the server classified its request and report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The published path of the page ([`Dim::Path`]).
    pub path: String,
    /// The referring host ([`referrer`]), `""` for none.
    pub referrer: String,
    /// The country ([`country`]).
    pub country: String,
    pub device: Device,
    /// The version viewed.
    pub version: u32,
    /// Whether the page was in Clax's frame.
    pub framed: bool,
    pub load_ms: Option<u32>,
    pub lcp_ms: Option<u32>,
}

/// What the day's seen set says of a view's visitor: whether this is their
/// first view of the day on the artifact's site, and their first of the
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Seen {
    pub first_of_day: bool,
    pub first_on_page: bool,
}

/// Views and visitors of one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub views: u64,
    pub visitors: u64,
}

impl Counts {
    /// Adds `other`'s views and visitors.
    pub fn add(&mut self, other: Counts) {
        self.views = self.views.saturating_add(other.views);
        self.visitors = self.visitors.saturating_add(other.visitors);
    }
}

/// One artifact's sums for one day: what `analytics_days`,
/// `analytics_dims` and `analytics_timings` hold for it, or what a batch
/// adds to them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DaySums {
    pub views: u64,
    pub visitors: u64,
    /// Views in Clax's frame.
    pub framed: u64,
    pub dims: BTreeMap<(Dim, String), Counts>,
    pub timings: BTreeMap<(Metric, u8), u64>,
}

impl DaySums {
    /// Counts `view`, whose visitor the seen set describes as `seen`: one
    /// view under each dimension; one visitor for the day, and under the
    /// country, device, referring host and version, on the visitor's first
    /// view of the day; one visitor of the page on their first view of it.
    pub fn add(&mut self, view: &View, seen: Seen) {
        self.views += 1;
        if view.framed {
            self.framed += 1;
        }
        if seen.first_of_day {
            self.visitors += 1;
        }
        let first = u64::from(seen.first_of_day);
        let mut put = |dim: Dim, value: String, visitors: u64| {
            self.dims
                .entry((dim, value))
                .or_default()
                .add(Counts { views: 1, visitors });
        };
        put(Dim::Path, view.path.clone(), u64::from(seen.first_on_page));
        put(Dim::Referrer, view.referrer.clone(), first);
        put(Dim::Country, view.country.clone(), first);
        put(Dim::Device, view.device.as_str().to_string(), first);
        put(Dim::Version, view.version.to_string(), first);
        for (metric, ms) in [(Metric::Load, view.load_ms), (Metric::Lcp, view.lcp_ms)] {
            if let Some(ms) = ms {
                *self.timings.entry((metric, bucket(ms))).or_default() += 1;
            }
        }
    }

    /// Adds every count of `other`.
    pub fn merge(&mut self, other: &DaySums) {
        self.views = self.views.saturating_add(other.views);
        self.visitors = self.visitors.saturating_add(other.visitors);
        self.framed = self.framed.saturating_add(other.framed);
        for (k, c) in &other.dims {
            self.dims.entry(k.clone()).or_default().add(*c);
        }
        for (k, n) in &other.timings {
            let e = self.timings.entry(*k).or_default();
            *e = e.saturating_add(*n);
        }
    }

    /// Whether it counts nothing.
    pub fn is_empty(&self) -> bool {
        self.views == 0 && self.dims.is_empty() && self.timings.is_empty()
    }
}

/// A visitor's row in the day's seen set: their 16-byte hash and the page
/// they viewed, or `""` for their own row (a view of any page that day).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeenRow {
    pub day: String,
    pub visitor: [u8; 16],
    pub path: String,
}

/// Views not yet written: sums keyed by artifact ID and day, and the seen
/// set's rows they added. Written in one store transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Batch {
    pub sums: BTreeMap<(String, String), DaySums>,
    pub seen: Vec<SeenRow>,
}

impl Batch {
    /// Whether it holds nothing to write.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty() && self.sums.values().all(DaySums::is_empty)
    }
}

/// One day of a range's totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayTotals {
    pub day: String,
    pub views: u64,
    pub visitors: u64,
    pub framed: u64,
}

/// A row of a dimension's table in a range: a value and its counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimRow {
    pub value: String,
    pub counts: Counts,
}

/// A version published in a range: its number and when (`versions.created_at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMark {
    pub n: u32,
    pub published_at: String,
}

/// An artifact's totals over a range of days.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RangeTotals {
    /// Every day of the range, oldest first, zeros included.
    pub days: Vec<DayTotals>,
    /// The versions published in the range, oldest first.
    pub versions: Vec<VersionMark>,
    /// Each dimension's values summed over the range.
    pub dims: BTreeMap<Dim, BTreeMap<String, Counts>>,
    /// Each metric's histogram merged over the range.
    pub timings: BTreeMap<Metric, Histogram>,
}

impl RangeTotals {
    /// Adds `sums`, counted on `day`, to the range: a batch not yet
    /// written. A day outside the range adds nothing.
    pub fn merge_day(&mut self, day: &str, sums: &DaySums) {
        let Some(d) = self.days.iter_mut().find(|d| d.day == day) else {
            return;
        };
        d.views = d.views.saturating_add(sums.views);
        d.visitors = d.visitors.saturating_add(sums.visitors);
        d.framed = d.framed.saturating_add(sums.framed);
        for ((dim, value), c) in &sums.dims {
            self.dims
                .entry(*dim)
                .or_default()
                .entry(value.clone())
                .or_default()
                .add(*c);
        }
        for (&(metric, b), &n) in &sums.timings {
            self.timings.entry(metric).or_default().add(b, n);
        }
    }

    /// The sum of the days' views, visitors and views in Clax's frame (a
    /// visitor on two days counts twice).
    pub fn totals(&self) -> DayTotals {
        let mut t = DayTotals {
            day: String::new(),
            views: 0,
            visitors: 0,
            framed: 0,
        };
        for d in &self.days {
            t.views = t.views.saturating_add(d.views);
            t.visitors = t.visitors.saturating_add(d.visitors);
            t.framed = t.framed.saturating_add(d.framed);
        }
        t
    }

    /// The median of `metric` over the range, `None` without samples.
    pub fn median(&self, metric: Metric) -> Option<u32> {
        self.timings.get(&metric).and_then(Histogram::median)
    }

    /// `dim`'s top `n` rows by views (ties by value), the rest summed into
    /// one [`OTHER`] row (with any [`OTHER`] the store already held).
    pub fn top(&self, dim: Dim, n: usize) -> Vec<DimRow> {
        let Some(values) = self.dims.get(&dim) else {
            return Vec::new();
        };
        let mut rows: Vec<DimRow> = values
            .iter()
            .filter(|(v, _)| v.as_str() != OTHER)
            .map(|(v, c)| DimRow {
                value: v.clone(),
                counts: *c,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.counts
                .views
                .cmp(&a.counts.views)
                .then_with(|| a.value.cmp(&b.value))
        });
        let mut other = values.get(OTHER).copied().unwrap_or_default();
        for r in rows.iter().skip(n) {
            other.add(r.counts);
        }
        rows.truncate(n);
        if other != Counts::default() {
            rows.push(DimRow {
                value: OTHER.to_string(),
                counts: other,
            });
        }
        rows
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn device_classes_follow_the_hint_then_the_user_agent() {
        let cases: &[(&str, Option<&str>, Device)] = &[
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
                None,
                Device::Phone,
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36",
                None,
                Device::Phone,
            ),
            (
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36",
                Some("?1"),
                Device::Phone,
            ),
            (
                "Mozilla/5.0 (iPad; CPU OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1",
                None,
                Device::Tablet,
            ),
            (
                "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                Some("?0"),
                Device::Tablet,
            ),
            (
                "Mozilla/5.0 (Android 14; Tablet; rv:127.0) Gecko/127.0 Firefox/127.0",
                None,
                Device::Tablet,
            ),
            (
                "Mozilla/5.0 (Android 14; Mobile; rv:127.0) Gecko/127.0 Firefox/127.0",
                None,
                Device::Phone,
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                Some("?0"),
                Device::Desktop,
            ),
            // iPadOS Safari sends a Mac's User-Agent.
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
                None,
                Device::Desktop,
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0",
                None,
                Device::Desktop,
            ),
            // The hint wins over a desktop User-Agent.
            (
                "Mozilla/5.0 (X11; Linux x86_64)",
                Some(" ?1 "),
                Device::Phone,
            ),
        ];
        for (ua, hint, want) in cases {
            assert_eq!(device(ua, *hint), *want, "{ua} {hint:?}");
        }
    }

    #[test]
    fn bots_crawlers_previews_and_headless_browsers_are_not_viewers() {
        for ua in [
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)",
            "Mozilla/5.0 (compatible; Yahoo! Slurp; http://help.yahoo.com/help/us/ysearch/slurp)",
            "Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)",
            "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)",
            "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)",
            "Twitterbot/1.0",
            "WhatsApp/2.23.20.0",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/126.0.0.0 Safari/537.36",
            "Mozilla/5.0 (compatible; YandexSpider/3.0)",
            "Mozilla/5.0 AhrefsCrawler",
            "Mozilla/5.0 (Linux; Android 11; moto g power (2022)) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36 Chrome-Lighthouse",
            "",
            "   ",
        ] {
            assert!(is_bot(ua), "{ua:?}");
        }
        for ua in [
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:127.0) Gecko/20100101 Firefox/127.0",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0",
        ] {
            assert!(!is_bot(ua), "{ua}");
        }
    }

    #[test]
    fn countries_are_two_upper_case_characters_or_xx() {
        assert_eq!(country(Some("de")), "DE");
        assert_eq!(country(Some("US")), "US");
        assert_eq!(country(Some("T1")), "T1");
        assert_eq!(country(Some(" fr ")), "FR");
        for bad in [
            None,
            Some(""),
            Some("D"),
            Some("DEU"),
            Some("1T"),
            Some("D-"),
        ] {
            assert_eq!(country(bad), "XX", "{bad:?}");
        }
    }

    #[test]
    fn a_page_is_an_html_file_or_the_index() {
        assert!(is_page("a/b.html", "text/html"));
        assert!(is_page("a.htm", "Text/HTML; charset=utf-8"));
        assert!(is_page("index.html", "application/octet-stream"));
        assert!(!is_page("app.js", "text/javascript"));
        assert!(!is_page("notes.md", "text/markdown"));
    }

    #[test]
    fn a_referrer_is_a_foreign_host_name_or_nothing() {
        let own = ["notes.sites.clax.example", "clax.example"];
        let domain = Some("sites.clax.example");
        let r = |s: &str| referrer(s, &own, domain);
        assert_eq!(r("news.example.com"), "news.example.com");
        assert_eq!(r("News.Example.COM"), "news.example.com");
        assert_eq!(r("xn--bcher-kva.example"), "xn--bcher-kva.example");
        assert_eq!(r("localhost"), "localhost");
        let longest = format!("{}.com", "a".repeat(REFERRER_MAX - 4));
        assert_eq!(r(&longest), longest);
        for none in [
            "",
            "news.example.com/path",
            "news.example.com:8080",
            "https://news.example.com",
            "user@news.example.com",
            "news example.com",
            ".example.com",
            "example.com.",
            "a..b",
            "[::1]",
            "::1",
            "[2001:db8::1]",
            "10.0.0.1",
            "203.0.113.7",
            "3232235777",
            "0x7f.1",
            "bücher.example",
            "notes.sites.clax.example",
            "NOTES.sites.clax.example",
            "clax.example",
            "other.sites.clax.example",
            "sites.clax.example",
        ] {
            assert_eq!(r(none), "", "{none:?}");
        }
        assert_eq!(r(&format!("{}.com", "a".repeat(REFERRER_MAX - 3))), "");
        // Without a site domain, only the hosts given are this Clax's.
        assert_eq!(
            referrer("other.sites.clax.example", &own, None),
            "other.sites.clax.example"
        );
    }

    #[test]
    fn buckets_are_quarter_octaves_clamped_to_0_and_80() {
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(1), 0);
        // 2^(1/4) = 1.19: 2 is in 4, the bucket of 2^1 exactly.
        assert_eq!(bucket(2), 4);
        assert_eq!(bucket(3), 6); // 4·log2 3 = 6.34
        for k in 1..20u32 {
            assert_eq!(bucket(1 << k), u8::try_from(4 * k).unwrap(), "2^{k}");
        }
        // From 8 down, a millisecond less is the bucket below.
        for k in 3..20u32 {
            assert_eq!(
                bucket((1 << k) - 1),
                u8::try_from(4 * k - 1).unwrap(),
                "2^{k}-1"
            );
        }
        assert_eq!(bucket(840), 38); // 4·log2 840 = 38.85
        assert_eq!(bucket(600_000), 76);
        assert_eq!(bucket(1 << 20), MAX_BUCKET);
        assert_eq!(bucket(u32::MAX), MAX_BUCKET);
        // Each bucket's middle is within about 10 % of the times it holds.
        for ms in [2u32, 3, 10, 100, 840, 1210, 5000, 60_000, 600_000] {
            let mid = f64::from(bucket_middle(bucket(ms)));
            let off = (mid - f64::from(ms)).abs() / f64::from(ms);
            assert!(off < 0.11, "{ms}: {mid}");
            assert_eq!(bucket(bucket_middle(bucket(ms))), bucket(ms), "{ms}");
        }
    }

    #[test]
    fn the_median_is_read_from_merged_histograms() {
        assert_eq!(Histogram::default().median(), None);
        let mut a = Histogram::default();
        a.add(bucket(100), 3);
        let mut b = Histogram::default();
        b.add(bucket(1000), 2);
        b.add(bucket(5000), 2);
        a.merge(&b);
        assert_eq!(a.total(), 7);
        // The 4th of 7 views is in 1000's bucket.
        assert_eq!(a.median(), Some(bucket_middle(bucket(1000))));
        let mut even = Histogram::default();
        even.add(bucket(100), 1);
        even.add(bucket(1000), 1);
        // The lower middle of two.
        assert_eq!(even.median(), Some(bucket_middle(bucket(100))));
        let mut clamped = Histogram::default();
        clamped.add(200, 1);
        assert_eq!(clamped.buckets().collect::<Vec<_>>(), vec![(MAX_BUCKET, 1)]);
    }

    #[test]
    fn day_keys_are_utc_days_around_midnight_and_month_ends() {
        assert_eq!(day_key(at("2026-11-19T23:59:59.999Z")), "2026-11-19");
        assert_eq!(day_key(at("2026-11-20T00:00:00Z")), "2026-11-20");
        assert_eq!(day_key(at("2026-11-20T00:30:00+01:00")), "2026-11-19");
        assert_eq!(
            next_midnight(at("2026-11-19T10:00:00Z")),
            at("2026-11-20T00:00:00Z")
        );
        assert_eq!(
            next_midnight(at("2026-11-20T00:00:00Z")),
            at("2026-11-21T00:00:00Z")
        );
        assert_eq!(
            next_midnight(at("2026-12-31T23:59:59Z")),
            at("2027-01-01T00:00:00Z")
        );
        assert_eq!(
            next_midnight(at("2028-02-28T12:00:00Z")),
            at("2028-02-29T00:00:00Z")
        );
        assert_eq!(
            parse_day("2026-02-28"),
            NaiveDate::from_ymd_opt(2026, 2, 28)
        );
        for bad in [
            "2026-02-30",
            "2026-2-28",
            "20260228",
            "2026-02-28T00:00",
            "",
        ] {
            assert_eq!(parse_day(bad), None, "{bad}");
        }
        let d = |s: &str| parse_day(s).unwrap();
        assert_eq!(oldest_kept(d("2026-11-19")), d("2025-10-19"));
        assert_eq!(oldest_kept(d("2026-01-31")), d("2024-12-31"));
        // The month 13 back is shorter: its last day.
        assert_eq!(oldest_kept(d("2026-03-31")), d("2025-02-28"));
        assert_eq!(oldest_kept(d("2029-03-29")), d("2028-02-29"));
    }

    fn view(path: &str, version: u32) -> View {
        View {
            path: path.into(),
            referrer: String::new(),
            country: "DE".into(),
            device: Device::Phone,
            version,
            framed: false,
            load_ms: Some(840),
            lcp_ms: None,
        }
    }

    #[test]
    fn a_view_counts_its_visitor_once_a_day_and_once_a_page() {
        let mut s = DaySums::default();
        s.add(
            &view("index.html", 2),
            Seen {
                first_of_day: true,
                first_on_page: true,
            },
        );
        s.add(
            &View {
                framed: true,
                ..view("index.html", 3)
            },
            Seen::default(),
        );
        s.add(
            &view("b.html", 3),
            Seen {
                first_of_day: false,
                first_on_page: true,
            },
        );
        assert_eq!((s.views, s.visitors, s.framed), (3, 1, 1));
        let c = |dim: Dim, v: &str| s.dims[&(dim, v.to_string())];
        assert_eq!(
            c(Dim::Path, "index.html"),
            Counts {
                views: 2,
                visitors: 1
            }
        );
        assert_eq!(
            c(Dim::Path, "b.html"),
            Counts {
                views: 1,
                visitors: 1
            }
        );
        // The visitor is attributed to their first view's version alone.
        assert_eq!(
            c(Dim::Version, "2"),
            Counts {
                views: 1,
                visitors: 1
            }
        );
        assert_eq!(
            c(Dim::Version, "3"),
            Counts {
                views: 2,
                visitors: 0
            }
        );
        assert_eq!(
            c(Dim::Country, "DE"),
            Counts {
                views: 3,
                visitors: 1
            }
        );
        assert_eq!(
            c(Dim::Device, "phone"),
            Counts {
                views: 3,
                visitors: 1
            }
        );
        assert_eq!(
            c(Dim::Referrer, ""),
            Counts {
                views: 3,
                visitors: 1
            }
        );
        assert_eq!(s.timings[&(Metric::Load, bucket(840))], 3);
        assert!(!s.timings.keys().any(|(m, _)| *m == Metric::Lcp));
        let mut twice = s.clone();
        twice.merge(&s);
        assert_eq!((twice.views, twice.visitors), (6, 2));
        assert_eq!(twice.timings[&(Metric::Load, bucket(840))], 6);
    }

    #[test]
    fn a_ranges_top_rows_sum_the_rest_into_other() {
        let mut r = RangeTotals {
            days: vec![DayTotals {
                day: "2026-11-19".into(),
                views: 0,
                visitors: 0,
                framed: 0,
            }],
            ..RangeTotals::default()
        };
        let mut s = DaySums::default();
        for (i, path) in ["a", "b", "c", "d"].iter().enumerate() {
            for _ in 0..=i {
                s.add(&view(path, 1), Seen::default());
            }
        }
        s.dims.insert(
            (Dim::Path, OTHER.into()),
            Counts {
                views: 1,
                visitors: 1,
            },
        );
        r.merge_day("2026-11-19", &s);
        r.merge_day("2026-11-18", &s);
        let top = r.top(Dim::Path, 2);
        let got: Vec<_> = top
            .iter()
            .map(|r| (r.value.as_str(), r.counts.views))
            .collect();
        assert_eq!(got, vec![("d", 4), ("c", 3), (OTHER, 4)]);
        assert_eq!(r.totals().views, 10);
        assert_eq!(r.median(Metric::Load), Some(bucket_middle(bucket(840))));
        assert_eq!(r.median(Metric::Lcp), None);
        assert!(r.top(Dim::Referrer, 50).len() == 1);
        assert!(r.top(Dim::Country, 0).iter().all(|r| r.value == OTHER));
    }
}
