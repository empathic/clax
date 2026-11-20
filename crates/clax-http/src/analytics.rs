//! Counting page views on a hosted Clax: the report a page's bridge sends
//! to its own site (`POST /_clax/api/analytics`, read by [`read_report`]),
//! the daily visitor hash ([`visitor`]), and the views not yet written
//! ([`Analytics`], an [`AppState`](crate::AppState) field that counts only
//! while the state is hosted).
//!
//! - **Views are batched.** [`Analytics::count`] adds a view to sums kept in
//!   memory, keyed by artifact, day and dimension value; a batch is written
//!   when it is [`MAX_AGE`] old, when it holds [`MAX_VIEWS`] views, or at
//!   the day's rollover: from the request that finds it due, and from
//!   [`Analytics::sweep`] (`AppState::sweep`), whose due time
//!   [`Analytics::next_due_in`] gives `AppState::next_sweep_in`. A batch is
//!   written in transactions of at most [`ROWS_PER_TX`] rows, within the
//!   Worker's transaction cap.
//! - **Unique visitors** are counted with a hash of the day's secret
//!   (`Store::analytics_day_secret`), the artifact, the client's address
//!   key and its User-Agent; the address and User-Agent are dropped with
//!   the request. The day's seen set is held in memory, and a hash missing
//!   from memory is looked up in the store before it is taken as new, so a
//!   process woken mid-day counts no one twice.
//! - **The rollover**: at the first sweep (or view) after midnight UTC, the
//!   old day's batch is written and the old days' secrets and seen rows are
//!   deleted (`Store::analytics_rollover`, in bounded passes, the next at
//!   the next sweep), and one retention pass runs ([`retention`]).
//! - **What a process loses** when it stops or is evicted: the views of the
//!   batch not yet written, at most [`MAX_AGE`] of them, with their seen
//!   rows, so they are lost whole, never half-counted.

use crate::error::ApiError;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use clax_core::ArtifactId;
use clax_core::analytics::{Batch, DaySums, Seen, SeenRow, View, day_key, next_midnight};
use clax_store::Store;
use clax_store::analytics::Pass;
use serde::Deserialize;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

/// The most bytes a report's body may hold.
pub const MAX_BODY: usize = 2048;
/// The longest published path a report may name, in bytes.
pub const FILE_MAX: usize = 512;
/// The longest timing a report may carry, in milliseconds.
pub const TIMING_MAX: i64 = 600_000;
/// A batch is written once its oldest view is this old.
pub const MAX_AGE: Duration = Duration::from_secs(60);
/// A batch is written once it holds this many views.
pub const MAX_VIEWS: usize = 1000;
/// The most rows one transaction of a batch's write, a rollover pass or a
/// retention pass writes or deletes: each touches a page or two of 4 KiB,
/// far within the hosted Worker's 16 MiB a transaction.
pub const ROWS_PER_TX: usize = 1000;
/// The most seen-set entries kept in memory between writes; past it, the
/// set is emptied once its rows are all written, and lookups go to the
/// store.
const SEEN_KEPT: usize = 100_000;

/// A page's report, as the bridge sends it (`web/bridge/src/count.ts`).
/// Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    /// The published path of the page.
    pub file: String,
    /// The version served.
    pub version: i64,
    /// Whether the page was in a frame (Clax's, as `frame-ancestors` allows).
    #[serde(default)]
    pub framed: bool,
    /// The referring page's host alone, `""` for none.
    #[serde(default)]
    pub referrer: String,
    #[serde(default)]
    pub load_ms: Option<i64>,
    #[serde(default)]
    pub lcp_ms: Option<i64>,
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        format!("a page view's report holds at most {MAX_BODY} bytes"),
    )
}

/// The report in a request with `headers` and `body`: 413 `body_too_large`
/// past [`MAX_BODY`] bytes, 400 `invalid_argument` when it is not a report.
///
/// # Errors
/// As above.
pub async fn read_report(headers: &HeaderMap, body: axum::body::Body) -> Result<Report, ApiError> {
    let declared = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY as u64) {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, MAX_BODY)
        .await
        .map_err(|_| too_large())?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::bad_request("invalid_argument", format!("not a page view: {e}")))
}

/// What the request tells of a view beside its report: the client's
/// User-Agent and `Sec-CH-UA-Mobile`, its country ([`clax_core::analytics::country`],
/// when trusted), and the hosts that are this Clax's
/// ([`clax_core::analytics::referrer`]).
#[derive(Debug, Clone)]
pub struct Context<'a> {
    pub user_agent: &'a str,
    pub ch_mobile: Option<&'a str>,
    pub country: String,
    pub own_hosts: &'a [&'a str],
    pub site_domain: Option<&'a str>,
}

/// The view `report` describes, classified with `cx`; `None` when it is
/// not to be counted (a path longer than [`FILE_MAX`] or holding a control
/// character, or a version below 1). Whether the version exists is
/// [`Analytics::count`]'s to check. A timing outside 0 to [`TIMING_MAX`]
/// is dropped, the view kept.
pub fn view_of(report: &Report, cx: &Context<'_>) -> Option<View> {
    let file = &report.file;
    if file.is_empty() || file.len() > FILE_MAX || file.chars().any(char::is_control) {
        return None;
    }
    let version = u32::try_from(report.version).ok().filter(|v| *v >= 1)?;
    let timing = |t: Option<i64>| {
        t.filter(|ms| (0..=TIMING_MAX).contains(ms))
            .and_then(|ms| u32::try_from(ms).ok())
    };
    Some(View {
        path: file.clone(),
        referrer: clax_core::analytics::referrer(&report.referrer, cx.own_hosts, cx.site_domain),
        country: cx.country.clone(),
        device: clax_core::analytics::device(cx.user_agent, cx.ch_mobile),
        version,
        framed: report.framed,
        load_ms: timing(report.load_ms),
        lcp_ms: timing(report.lcp_ms),
    })
}

/// The visitor a view of artifact `aid` from address key `client` (a
/// client's address as its attempts are counted) with User-Agent `ua` is,
/// on the day whose secret is `secret`: the first 16 bytes of
/// `HMAC-SHA-256(secret, aid ‖ 0x00 ‖ client ‖ 0x00 ‖ ua)`.
pub fn visitor(secret: &[u8; 32], aid: &str, client: IpAddr, ua: &str) -> [u8; 16] {
    let mut msg = Vec::with_capacity(aid.len() + ua.len() + 48);
    msg.extend_from_slice(aid.as_bytes());
    msg.push(0);
    msg.extend_from_slice(client.to_string().as_bytes());
    msg.push(0);
    msg.extend_from_slice(ua.as_bytes());
    let h = clax_core::secret::hmac_sha256(secret, &msg);
    let mut v = [0u8; 16];
    v.copy_from_slice(&h[..16]);
    v
}

/// Up to `passes` retention passes (`Store::analytics_prune`, each of at
/// most [`ROWS_PER_TX`] rows), one after another while each leaves more:
/// what they deleted together, and whether rows are left.
///
/// # Errors
/// The store's.
pub fn retention(st: &Store, passes: usize) -> clax_core::Result<Pass> {
    let mut total = Pass::default();
    for _ in 0..passes {
        let p = st.analytics_prune(ROWS_PER_TX)?;
        total.deleted += p.deleted;
        total.more = p.more;
        if !p.more {
            break;
        }
    }
    Ok(total)
}

/// `batch` cut into batches of at most `rows` rows each (a day's totals, a
/// dimension value, a timing bucket and a seen row are a row each), in
/// order, so each is written in a transaction of its own.
pub fn split(batch: Batch, rows: usize) -> Vec<Batch> {
    let rows = rows.max(1);
    let mut parts = Vec::new();
    let mut cur = Batch::default();
    let mut n = 0usize;
    let room = |cur: &mut Batch, n: &mut usize, parts: &mut Vec<Batch>| {
        if *n >= rows {
            parts.push(std::mem::take(cur));
            *n = 0;
        }
        *n += 1;
    };
    for (key, sums) in batch.sums {
        room(&mut cur, &mut n, &mut parts);
        let e = cur.sums.entry(key.clone()).or_default();
        (e.views, e.visitors, e.framed) = (sums.views, sums.visitors, sums.framed);
        for (k, c) in sums.dims {
            room(&mut cur, &mut n, &mut parts);
            cur.sums
                .entry(key.clone())
                .or_default()
                .dims
                .entry(k)
                .or_default()
                .add(c);
        }
        for (k, c) in sums.timings {
            room(&mut cur, &mut n, &mut parts);
            *cur.sums
                .entry(key.clone())
                .or_default()
                .timings
                .entry(k)
                .or_default() += c;
        }
    }
    for row in batch.seen {
        room(&mut cur, &mut n, &mut parts);
        cur.seen.push(row);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// The views not yet written and the day's seen set in memory (see the
/// module docs). [`Analytics::default`] writes a batch [`MAX_AGE`] old;
/// [`Analytics::with_max_age`] at another age.
pub struct Analytics {
    inner: Mutex<Inner>,
    /// How old a batch is written ([`MAX_AGE`] but in a test host).
    max_age: Duration,
}

impl Default for Analytics {
    fn default() -> Self {
        Self::with_max_age(MAX_AGE)
    }
}

#[derive(Default)]
struct Inner {
    /// The views not yet written, and the seen rows they added.
    batch: Batch,
    /// How many views `batch` holds.
    views: usize,
    /// When `batch`'s oldest view was counted.
    since: Option<DateTime<Utc>>,
    /// The day `seen` and `secret` are of.
    day: String,
    /// `day`'s seen set as far as it is known here: (visitor, path) pairs,
    /// `""` for the visitor's own row.
    seen: HashSet<([u8; 16], String)>,
    /// `day`'s secret, once read.
    secret: Option<[u8; 32]>,
    /// The earliest day whose secret or seen rows are stored
    /// (`Store::analytics_open_day`); `None` until it is read.
    stored_open: Option<Option<String>>,
}

impl Inner {
    /// Forgets the seen set and secret of a day before `today`.
    fn turn_to(&mut self, today: &str) {
        if self.day != today {
            self.day = today.to_string();
            self.seen.clear();
            self.secret = None;
        }
    }

    /// Whether a day before `today` is still open: its secret or seen rows
    /// stored, or its views or seen rows not yet written.
    fn rollover_due(&self, today: &str) -> bool {
        matches!(&self.stored_open, Some(Some(d)) if d.as_str() < today)
            || self.batch.sums.keys().any(|(_, d)| d.as_str() < today)
            || self.batch.seen.iter().any(|r| r.day.as_str() < today)
    }

    /// Whether the batch is to be written at `now`: its age (`max_age`)
    /// or size.
    fn batch_due(&self, now: DateTime<Utc>, max_age: Duration) -> bool {
        self.views >= MAX_VIEWS
            || self
                .since
                .is_some_and(|t| (now - t).to_std().unwrap_or(Duration::ZERO) >= max_age)
    }
}

impl Analytics {
    /// Views not yet written, kept until the oldest is `max_age` old (a
    /// test host's shorter age; [`MAX_AGE`] otherwise).
    pub fn with_max_age(max_age: Duration) -> Self {
        Self {
            inner: Mutex::default(),
            max_age,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Counts a view of artifact `aid` from address key `client` with
    /// User-Agent `ua`, in the store's time: nothing when the artifact is
    /// not counted (`Store::analytics_on`: missing, deleted, a draft, or
    /// its setting off), when `view`'s version is past its current one, or
    /// when `view`'s path is not one of that version's pages
    /// ([`clax_core::analytics::is_page`]): a path a page cannot be served
    /// at never reaches the seen set or the Pages table. Looking it up
    /// reads the version's row once (`Store::file_meta`), as the page's own
    /// load did. Then writes the batch if that made it due. True when the
    /// view was counted.
    ///
    /// # Errors
    /// The store's.
    pub fn count(
        &self,
        st: &Store,
        aid: &ArtifactId,
        client: IpAddr,
        ua: &str,
        view: View,
    ) -> clax_core::Result<bool> {
        if !st.analytics_on(aid)? {
            return Ok(false);
        }
        let Some(a) = st.get_artifact(aid)? else {
            return Ok(false);
        };
        if view.version > a.current_version {
            return Ok(false);
        }
        let page = st
            .file_meta(aid, view.version, &view.path)?
            .is_some_and(|m| clax_core::analytics::is_page(&view.path, &m.content_type));
        if !page {
            return Ok(false);
        }
        let now = st.now_utc();
        let today = day_key(now);
        let secret = {
            let mut g = self.lock();
            g.turn_to(&today);
            g.secret
        };
        let secret = match secret {
            Some(s) => s,
            None => {
                let s = st.analytics_day_secret(&today)?;
                let mut g = self.lock();
                if g.day == today {
                    g.secret = Some(s);
                }
                // Today's secret is stored now.
                match &mut g.stored_open {
                    Some(open @ None) => *open = Some(today.clone()),
                    Some(Some(d)) if today < *d => *d = today.clone(),
                    _ => {}
                }
                s
            }
        };
        let v = visitor(&secret, aid.as_str(), client, ua);
        let own = (v, String::new());
        let page = (v, view.path.clone());
        let known = {
            let g = self.lock();
            g.day == today && g.seen.contains(&own) && g.seen.contains(&page)
        };
        // Missing from memory: the table may know it (an earlier process's).
        let stored = if known {
            Seen::default()
        } else {
            st.analytics_seen(&today, &v, &view.path)?
        };
        let mut g = self.lock();
        g.turn_to(&today);
        let seen = Seen {
            first_of_day: stored.first_of_day && !g.seen.contains(&own),
            first_on_page: stored.first_on_page && !g.seen.contains(&page),
        };
        for (first, key) in [(seen.first_of_day, own), (seen.first_on_page, page)] {
            if g.seen.insert(key.clone()) && first {
                g.batch.seen.push(SeenRow {
                    day: today.clone(),
                    visitor: key.0,
                    path: key.1,
                });
            }
        }
        g.batch
            .sums
            .entry((aid.as_str().to_string(), today))
            .or_default()
            .add(&view, seen);
        g.views += 1;
        g.since.get_or_insert(now);
        drop(g);
        self.write_due(st, now)?;
        Ok(true)
    }

    /// Writes what is due at the store's time: the rollover, when a day
    /// before today is still open; the batch, when it is due. Reads the
    /// stored open day first if it was not read since start.
    ///
    /// # Errors
    /// The store's.
    pub fn sweep(&self, st: &Store) -> clax_core::Result<()> {
        if self.lock().stored_open.is_none() {
            let open = st.analytics_open_day()?;
            let mut g = self.lock();
            if g.stored_open.is_none() {
                g.stored_open = Some(open);
            }
        }
        self.write_due(st, st.now_utc())
    }

    /// How long from `now` until [`Analytics::sweep`] has something to do:
    /// at once while the stored open day was not read since start or a
    /// rollover is due; else the earliest of the batch's age limit
    /// ([`Analytics::with_max_age`]; at once
    /// when it is full) and the next midnight while a day is open (a
    /// secret or seen rows stored, or views not yet written). `None` with
    /// nothing to do.
    pub fn next_due_in(&self, now: DateTime<Utc>) -> Option<Duration> {
        let g = self.lock();
        let today = day_key(now);
        if g.stored_open.is_none() || g.rollover_due(&today) || g.views >= MAX_VIEWS {
            return Some(Duration::ZERO);
        }
        let until = |t: DateTime<Utc>| (t - now).to_std().unwrap_or(Duration::ZERO);
        let age = g
            .since
            .map(|t| until(t + chrono::Duration::from_std(self.max_age).unwrap_or_default()));
        let open = matches!(g.stored_open, Some(Some(_))) || !g.batch.is_empty();
        let midnight = open.then(|| until(next_midnight(now)));
        age.into_iter().chain(midnight).min()
    }

    /// The views of artifact `aid` not yet written, by day: what a read of
    /// its totals adds to what the store holds.
    pub fn pending(&self, aid: &str) -> Vec<(String, DaySums)> {
        self.lock()
            .batch
            .sums
            .iter()
            .filter(|((a, _), _)| a == aid)
            .map(|((_, d), s)| (d.clone(), s.clone()))
            .collect()
    }

    /// [`Analytics::sweep`]'s and [`Analytics::count`]'s writes at `now`.
    fn write_due(&self, st: &Store, now: DateTime<Utc>) -> clax_core::Result<()> {
        let today = day_key(now);
        let (batch, rollover) = {
            let mut g = self.lock();
            let rollover = g.rollover_due(&today);
            if !rollover && !g.batch_due(now, self.max_age) {
                return Ok(());
            }
            g.turn_to(&today);
            let batch = std::mem::take(&mut g.batch);
            g.views = 0;
            g.since = None;
            (batch, rollover)
        };
        let written = if rollover {
            self.roll_over(st, batch, &today)
        } else {
            split(batch, ROWS_PER_TX)
                .iter()
                .try_for_each(|part| st.analytics_record(part))
        };
        let mut g = self.lock();
        // Everything counted so far is in the store: lookups may go there.
        if g.seen.len() > SEEN_KEPT && g.batch.is_empty() {
            g.seen.clear();
        }
        drop(g);
        written
    }

    /// The rollover pass: writes `batch` (the old days' seen rows left
    /// out, which the pass deletes), deletes the old days' secrets and at
    /// most [`ROWS_PER_TX`] of their seen rows, runs one retention pass,
    /// and reads the open day again.
    fn roll_over(&self, st: &Store, mut batch: Batch, today: &str) -> clax_core::Result<()> {
        batch.seen.retain(|r| r.day.as_str() >= today);
        let mut parts = split(batch, ROWS_PER_TX);
        let last = parts.pop().unwrap_or_default();
        for part in &parts {
            st.analytics_record(part)?;
        }
        st.analytics_rollover(&last, ROWS_PER_TX)?;
        st.analytics_prune(ROWS_PER_TX)?;
        let open = st.analytics_open_day()?;
        self.lock().stored_open = Some(open);
        Ok(())
    }
}

/// Whether `headers` carry Global Privacy Control (`Sec-GPC: 1`).
pub fn sends_gpc(headers: &HeaderMap) -> bool {
    headers
        .get("sec-gpc")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "1")
}

/// The host name of `origin` (`https://a.example:8443` → `a.example`).
pub fn host_of(origin: &str) -> Option<String> {
    url::Url::parse(origin).ok()?.host_str().map(|h| {
        h.trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase()
    })
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::analytics::{Device, Dim};
    use clax_core::audit::AuditCtx;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn store(clock: Arc<ManualClock>) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock).unwrap();
        (dir, st)
    }

    fn artifact(st: &Store) -> ArtifactId {
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": "Doc",
                "files": {
                    "index.html": {"content": "<p>", "encoding": "utf8"},
                    "b.html": {"content": "<p>", "encoding": "utf8"},
                    "p0.html": {"content": "<p>", "encoding": "utf8"},
                    "p1.html": {"content": "<p>", "encoding": "utf8"},
                    "p2.html": {"content": "<p>", "encoding": "utf8"},
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let (a, _) = st.create_artifact(&AuditCtx::DAEMON, p, None).unwrap();
        ArtifactId::parse(&a.id).unwrap()
    }

    fn view(path: &str) -> View {
        View {
            path: path.into(),
            referrer: String::new(),
            country: "DE".into(),
            device: Device::Desktop,
            version: 1,
            framed: false,
            load_ms: Some(840),
            lcp_ms: None,
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn day(st: &Store, aid: &ArtifactId, d: &str) -> (u64, u64) {
        let d = clax_core::analytics::parse_day(d).unwrap();
        let r = st.analytics_range(aid, d, d).unwrap();
        (r.days[0].views, r.days[0].visitors)
    }

    const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0";

    #[test]
    fn the_hash_is_stable_within_a_day_and_differs_across_days_sites_and_clients() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let v = visitor(&a, "7q3k9mzx2b4t", ip("192.0.2.1"), UA);
        assert_eq!(v, visitor(&a, "7q3k9mzx2b4t", ip("192.0.2.1"), UA));
        assert_ne!(v, visitor(&b, "7q3k9mzx2b4t", ip("192.0.2.1"), UA), "a day");
        assert_ne!(
            v,
            visitor(&a, "8q3k9mzx2b4t", ip("192.0.2.1"), UA),
            "a site"
        );
        assert_ne!(
            v,
            visitor(&a, "7q3k9mzx2b4t", ip("192.0.2.2"), UA),
            "an address"
        );
        assert_ne!(
            v,
            visitor(&a, "7q3k9mzx2b4t", ip("192.0.2.1"), "x"),
            "a browser"
        );
        // The separators keep the fields apart.
        assert_ne!(
            visitor(&a, "ab", ip("192.0.2.1"), "c"),
            visitor(&a, "a", ip("192.0.2.1"), "bc")
        );
    }

    #[test]
    fn a_report_is_checked_and_normalized() {
        let own = ["site.example"];
        let cx = Context {
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) Mobile/15E148",
            ch_mobile: None,
            country: "DE".into(),
            own_hosts: &own,
            site_domain: None,
        };
        let r = |v: serde_json::Value| -> Report { serde_json::from_value(v).unwrap() };
        let v = view_of(
            &r(
                serde_json::json!({"file": "index.html", "version": 3, "framed": true,
                "referrer": "News.Example.com", "load_ms": 840, "lcp_ms": null, "extra": 1}),
            ),
            &cx,
        )
        .unwrap();
        assert_eq!(
            v,
            View {
                path: "index.html".into(),
                referrer: "news.example.com".into(),
                country: "DE".into(),
                device: Device::Phone,
                version: 3,
                framed: true,
                load_ms: Some(840),
                lcp_ms: None,
            }
        );
        let v = view_of(
            &r(serde_json::json!({"file": "a.html", "version": 1,
                "referrer": "site.example", "load_ms": 600_001, "lcp_ms": -1})),
            &cx,
        )
        .unwrap();
        assert_eq!((v.referrer.as_str(), v.load_ms, v.lcp_ms), ("", None, None));
        // An IP address is no referring site: it is never stored.
        for ip in ["192.0.2.1", "[2001:db8::1]"] {
            let v = view_of(
                &r(serde_json::json!({"file": "a.html", "version": 1, "referrer": ip})),
                &cx,
            )
            .unwrap();
            assert_eq!(v.referrer, "", "{ip}");
        }
        for bad in [
            serde_json::json!({"file": "", "version": 1}),
            serde_json::json!({"file": "a\nb.html", "version": 1}),
            serde_json::json!({"file": "a".repeat(FILE_MAX + 1), "version": 1}),
            serde_json::json!({"file": "a.html", "version": 0}),
            serde_json::json!({"file": "a.html", "version": -2}),
        ] {
            assert_eq!(view_of(&r(bad.clone()), &cx), None, "{bad}");
        }
        assert!(
            view_of(
                &r(serde_json::json!({"file": "a".repeat(FILE_MAX), "version": 1})),
                &cx
            )
            .is_some()
        );
    }

    #[test]
    fn a_batch_is_split_into_transactions_of_bounded_rows() {
        let mut b = Batch::default();
        let mut s = DaySums::default();
        for i in 0..5 {
            s.add(&view(&format!("p{i}.html")), Seen::default());
        }
        b.sums.insert(("a".into(), "2026-11-19".into()), s.clone());
        for i in 0..3u8 {
            b.seen.push(SeenRow {
                day: "2026-11-19".into(),
                visitor: [i; 16],
                path: String::new(),
            });
        }
        // 1 day row, 5 paths + 4 single-valued dimensions, 1 timing, 3 seen.
        let parts = split(b.clone(), 4);
        assert_eq!(parts.len(), 4);
        let mut merged = DaySums::default();
        let mut seen = 0;
        for p in &parts {
            let rows: usize = p
                .sums
                .values()
                .map(|s| 1 + s.dims.len() + s.timings.len())
                .sum::<usize>()
                + p.seen.len();
            assert!(rows <= 4 + p.sums.len(), "{rows}");
            for d in p.sums.values() {
                merged.merge(d);
            }
            seen += p.seen.len();
        }
        assert_eq!(merged, s);
        assert_eq!(seen, 3);
        assert_eq!(split(b.clone(), ROWS_PER_TX), vec![b]);
        assert!(split(Batch::default(), 4).is_empty());
    }

    #[test]
    fn the_batch_is_due_by_age_size_and_midnight_on_a_moved_clock() {
        let clock = Arc::new(ManualClock::at("2026-11-19T23:58:00Z"));
        let (_d, st) = store(clock.clone());
        let a = artifact(&st);
        let an = Analytics::default();
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::ZERO), "unread");
        an.sweep(&st).unwrap();
        assert_eq!(an.next_due_in(st.now_utc()), None, "nothing open");

        assert!(
            an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
                .unwrap()
        );
        assert_eq!(an.next_due_in(st.now_utc()), Some(MAX_AGE));
        assert_eq!(day(&st, &a, "2026-11-19"), (0, 0), "not yet written");
        assert_eq!(an.pending(a.as_str()).len(), 1);
        clock.advance(59);
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (0, 0));
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::from_secs(1)));
        clock.advance(1);
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::ZERO));
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (1, 1), "written at 60 s");
        assert!(an.pending(a.as_str()).is_empty());
        // Today's secret and seen rows are stored: due at midnight.
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::from_secs(60)));

        // A full batch is written by the view that fills it.
        for i in 0..MAX_VIEWS - 1 {
            an.count(
                &st,
                &a,
                ip("192.0.2.1"),
                UA,
                view(&format!("p{}.html", i % 3)),
            )
            .unwrap();
        }
        assert_eq!(day(&st, &a, "2026-11-19"), (1, 1));
        an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
            .unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (MAX_VIEWS as u64 + 1, 1));

        // Past midnight the rollover writes the old day and drops its secret.
        an.count(&st, &a, ip("192.0.2.9"), UA, view("index.html"))
            .unwrap();
        clock.advance(60);
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::ZERO));
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (MAX_VIEWS as u64 + 2, 2));
        assert_eq!(st.analytics_open_day().unwrap(), None);
        assert_eq!(an.next_due_in(st.now_utc()), None);
        // The same visitor on the new day is a new visitor.
        an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
            .unwrap();
        clock.advance(60);
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-20"), (1, 1));
    }

    #[test]
    fn a_shorter_max_age_writes_the_batch_sooner() {
        let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let a = artifact(&st);
        let an = Analytics::with_max_age(Duration::from_secs(2));
        an.sweep(&st).unwrap();
        an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
            .unwrap();
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::from_secs(2)));
        clock.advance(1);
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (0, 0));
        clock.advance(1);
        assert_eq!(an.next_due_in(st.now_utc()), Some(Duration::ZERO));
        an.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (1, 1));
    }

    #[test]
    fn a_view_missing_from_memory_but_in_the_table_is_not_counted_again() {
        let clock = Arc::new(ManualClock::at("2026-11-19T10:00:00Z"));
        let (_d, st) = store(clock.clone());
        let a = artifact(&st);
        let first = Analytics::default();
        first
            .count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
            .unwrap();
        clock.advance(60);
        first.sweep(&st).unwrap();
        // A process woken later has nothing in memory.
        let woken = Analytics::default();
        woken
            .count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
            .unwrap();
        woken
            .count(&st, &a, ip("192.0.2.1"), UA, view("b.html"))
            .unwrap();
        woken
            .count(&st, &a, ip("192.0.2.2"), UA, view("index.html"))
            .unwrap();
        clock.advance(60);
        woken.sweep(&st).unwrap();
        assert_eq!(day(&st, &a, "2026-11-19"), (4, 2));
        let d = clax_core::analytics::parse_day("2026-11-19").unwrap();
        let r = st.analytics_range(&a, d, d).unwrap();
        assert_eq!(r.dims[&Dim::Path]["index.html"].visitors, 2);
        assert_eq!(r.dims[&Dim::Path]["b.html"].visitors, 1);
    }

    #[test]
    fn nothing_is_counted_for_a_version_past_current_or_an_artifact_turned_off() {
        let (_d, st) = store(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let a = artifact(&st);
        let an = Analytics::default();
        let mut v = view("index.html");
        v.version = 2;
        assert!(!an.count(&st, &a, ip("192.0.2.1"), UA, v).unwrap());
        st.set_analytics(&AuditCtx::DAEMON, &a, false).unwrap();
        assert!(
            !an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
                .unwrap()
        );
        assert!(an.pending(a.as_str()).is_empty());
    }

    #[test]
    fn a_path_that_is_not_one_of_the_versions_pages_is_not_counted() {
        let (_d, st) = store(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": "Doc",
                "files": {
                    "index.html": {"content": "<p>", "encoding": "utf8"},
                    "guide/b.html": {"content": "<p>", "encoding": "utf8"},
                    "app.js": {"content": "1", "encoding": "utf8"},
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let (a, _) = st.create_artifact(&AuditCtx::DAEMON, p, None).unwrap();
        let a = ArtifactId::parse(&a.id).unwrap();
        let an = Analytics::default();
        for path in ["nope.html", "app.js", "<script>alert(1)</script>", "guide/"] {
            assert!(
                !an.count(&st, &a, ip("192.0.2.1"), UA, view(path)).unwrap(),
                "{path}"
            );
        }
        assert!(an.pending(a.as_str()).is_empty(), "nothing pending");
        assert!(an.lock().seen.is_empty() && an.lock().batch.seen.is_empty());
        assert!(
            an.count(&st, &a, ip("192.0.2.1"), UA, view("guide/b.html"))
                .unwrap()
        );
        assert!(
            an.count(&st, &a, ip("192.0.2.1"), UA, view("index.html"))
                .unwrap()
        );
    }

    #[test]
    fn retention_runs_passes_while_they_leave_more() {
        let (_d, st) = store(Arc::new(ManualClock::at("2026-11-19T10:00:00Z")));
        assert_eq!(retention(&st, 3).unwrap(), Pass::default());
    }
}
