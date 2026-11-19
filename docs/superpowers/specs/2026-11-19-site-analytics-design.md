# Site analytics on a hosted Clax

## Why

Whoever publishes a clax on a hosted Clax cannot tell whether anyone opened
it, from where, on what device, or how long it took to load. Outside
analytics cannot help: the lockdown (`crates/clax-http/src/content_policy.rs`,
`lockdown`) allows `connect-src 'self'` only, and a third-party script on
every published page is the wrong default for Clax anyway. The site already
has a first-party channel: every HTML page carries the bridge
(`web/bridge/src/bridge.ts`), which talks to its own site's
`/_clax/api/` (`crates/clax-server/src/routes/site_api.rs`).

## What changes

- The bridge reports one page view per page load to its own site,
  `POST /_clax/api/analytics`, with the page's load timing, unless the
  browser sends Global Privacy Control.
- The server adds what it knows from the request (country, device class,
  referring host, path, version, framed or direct) and counts the view
  into daily totals for the artifact. Unique visitors are counted with a
  daily-rotating hash, never a cookie or a stored address.
- Each artifact has a setting, `analytics`, `on` by default, which its
  administrator may turn `off` (and on again) without approval.
- The artifact's administrator reads the totals in an Analytics panel in
  the shell, through `GET /api/artifacts/<id>/analytics`.
- Only a hosted Clax counts (`AppState::hosted` is set: the Cloudflare
  Worker, `crates/clax-worker`, and a daemon with `[hosted]`,
  `crates/clax-http/src/hosted.rs`). A local Clax counts nothing, writes no
  marker in its pages and shows no Analytics UI.

## Goals

- A clax's administrator sees, per day: visitors, page views and load time,
  and over a range: the pages, referring sites, countries, devices and
  versions viewed.
- Nothing kept identifies a visitor: no address, no cookie read or set for
  analytics, no per-visitor row past the day it was made.
- A page can never count a view for another clax.
- A page view costs a viewer one small same-origin request after the page
  has loaded, and the eager bridge stays within a few hundred gzip bytes of
  its size today.

## Non-goals

- Named viewers ("who opened it"), even for a private site whose viewers
  are signed in.
- Events a page records itself. The storage leaves room for them
  (`analytics_dims.dim` is checked in Rust, not in SQL; see "Storage").
- An overview across claxen, exports, alerts, real-time counts.
- Any third-party script or host.
- Unique visitors over a range of days: a visitor on two days is two daily
  visitors (the hash rotates daily, so days cannot be joined).

## Anti-goals

- No path from a page, an agent or a forged request to a count on another
  artifact.
- No IP address, User-Agent string or visitor hash in the database after
  the day's rollover, in a log, or in the audit journal.
- No change to what a page may load or connect to (the lockdown, the
  `unrestricted` setting), and no new cookie.

## Behaviour

### What counts as a page view

The daemon marks a page for counting by an attribute on its bridge tag,
`data-count="1"`, written when all of these hold at serve time:

- the Clax is hosted (`s.hosted.is_some()`);
- the artifact's `analytics` setting is `on`;
- the page is the artifact's own HTML page, at its site's root or under
  `/v/<n>/`: not a draft (a hidden artifact, `artifacts.hidden = 1`), not
  a live page's snapshot (`s.live_ids`), not the Clax app's (`s.embed`).

The bridge reports only when the tag says so, so a local Clax and an
artifact turned off make no request at all. The server checks the same
conditions again on every report: the attribute saves requests, it grants
nothing.

The attribute is added to the wrapped page after the wrap cache
(`crates/clax-server/src/routes/content.rs`, `lookup`), whose entries stay
keyed by artifact, version and file; `crates/clax-core/src/wrap.rs` gains a
helper that inserts it before the tag's `></script>` as it does
`data-origin` and `data-api` (`with_count`). A page's `ETag` is of the bytes
sent (`http_cache::html`, and `tagged_by_policy` over them for an
`unrestricted` page), which carry the mark while counting, so a page a
browser revalidates after the setting changes is answered 200 with the
right tag, never 304; no `\0analytics=on` suffix is needed.

The bridge reports nothing:

- when `navigator.globalPrivacyControl` is true;
- in the app's inspect and migration frames (`INSPECT_FRAME`,
  `MIGRATE_FRAME`);
- for a document restored from the back/forward cache (one report per
  load, and a restore is no load);
- while the document is prerendering: it waits for `prerenderingchange`,
  and a prerender never shown is never reported.

The server counts nothing (and still answers 204, so a page learns nothing
about the rules) for a request with `Sec-GPC: 1`, a User-Agent that names a
bot, crawler, spider, preview fetcher or headless browser (a short fixed
list in `clax_core::analytics`, `is_bot`) or is empty, an artifact that is deleted, hidden or
`off`, a local Clax, or a client over its rate (below).

A page's own `history.pushState` routes are not page views: one load, one
report. A page that reloads in the shell (a republish's Reload) is a new
load and a new report.

### The report

Sent once, in the first task after the document's `load` event (at once
if the bridge runs after it), from `web/bridge/src/count.ts`:

```
POST /_clax/api/analytics
Content-Type: application/json

{"file": "index.html", "version": 3, "framed": false,
 "referrer": "news.example.com", "load_ms": 840, "lcp_ms": 1210}
```

- `file` and `version` are the bridge tag's (`readMeta`): the published
  path of the page and the version served.
- `framed` is the bridge's `framed` (`window.parent !== window` as it was
  when the bridge ran). `frame-ancestors` lets only the shell and the
  site's own pages frame it, so a framed view is a view in Clax.
- `referrer` is the host of `document.referrer` alone (`URL.hostname`), or
  `""`. The bridge never sends a path or query of it.
- `load_ms` is the navigation entry's `loadEventStart`
  (`performance.getEntriesByType("navigation")[0]`), rounded; `null`
  without one. For a prerendered page both timings count from its
  activation (`activationStart`), floored at 0.
- `lcp_ms` is the latest `largest-contentful-paint` entry seen by then
  through a `PerformanceObserver` with `buffered: true`, rounded; `null`
  where the browser has none (Safari). It is the paint as of load, not the
  final LCP, which needs a report at the page's end: the final value is
  given up for one report sent while the page is surely alive.

It is sent with `fetch(url, {method: "POST", keepalive: true, headers:
{"content-type": "application/json"}, body})`, captured with the timers
when the bridge runs, before any page script (a page replacing `fetch`
later cannot read the report, only stop or forge its own clax's, which is
accepted). Not `navigator.sendBeacon` with a string body: site responses
carry `Referrer-Policy: no-referrer`
(`crates/clax-http/src/site_access.rs`, `site_headers`), and under that
policy a `no-cors` POST (a beacon of a CORS-safelisted type) is sent with
`Origin: null`, which the site API's origin check refuses (403
`forbidden_origin`). A same-origin `fetch` in `cors` mode carries the real
origin. A failure is ignored: no retry, no console message.

### Where it lives in the bridge, and what it costs

The report goes in the eager bridge, not a lazy part. The eager bridge
measures 6348 gzip bytes on main's build of 2026-11-18
(`bazel-bin/web/dist/_clax/bridge.js`, 15400 bytes raw, gzip level 9)
against its 6732 budget (`web/perf/bundle-budget.json`, `bridge`): about
380 bytes of headroom. The report is about 650 bytes minified, sharing
most of its tokens with the bridge (`dataset`, `addEventListener`,
`performance`, `JSON.stringify`), which should land near 300 gzip bytes.
A part of its own (`web/bridge/src/parts/`, loaded on need) would keep the
eager bridge unchanged but add a module request to a page's first load and
a load that can fail after the page has loaded, for code that runs on
every counted page; parts exist for code most pages never run. Recorded as
an alternative with `revisit:` the eager bridge's budget can no longer
take it. If the measured growth exceeds the headroom, the `bridge` budget
is raised by hand to the measured size and recorded, as earlier raises
were (qualifier records on `web/perf/bundle-budget.json`); see Open
question 1. Measured with Task 2: the baseline had reached its 6732
budget by then, and the report added 521 gzip bytes (6732 to 7253, the
budget raised to 7253 and recorded).

`web/bridge/src/count.ts` exports one function taking its world (the
script tag's dataset, `navigator`, `document`, `performance`,
`PerformanceObserver`, the captured `fetch` and a `Clock` like
`part-loader.ts`'s) so its unit tests drive it without timers or a
browser.

### The endpoint

`POST /_clax/api/analytics` on the artifact's site host, a special case in
`site_api::answer` beside `whoami` (it is no public-host route, so it is
not in `inner_path`):

- **Which artifact:** the site host's (`SiteRequest`, from the `Host`
  header, `crates/clax-http/src/host.rs`). Nothing in the path or body
  names an artifact.
- **Who may report:** whoever the site's access check let in
  (`site_access_layer`): on a site open to the world, anyone (a
  `SiteCaller::Visitor`); on a private site, only a site session's holder,
  so a request without one is 401 `site_session_required` before it
  reaches the route and only someone who may read the artifact is counted.
  The caller's identity is never recorded: it decides access, nothing
  more. The visitor cookie (`__Host-clax_visitor`, a year, set by the
  runtime on world-open sites) is not read for analytics.
- **Origin:** the site API's existing check (`origin_ok`): a write must
  carry `Origin` equal to the site's own origin, else 403
  `forbidden_origin`. A page on another clax's site, even one at
  `unrestricted` that may fetch anywhere, sends its own origin and is
  refused; with the lockdown it cannot send at all. Until the site domain
  is on the Public Suffix List sibling sites are same-site
  (`docs/hosted/public-suffix.md`), which the origin check does not depend
  on.
- **Method and body:** `POST` only (405 otherwise); a JSON body of at most
  2 KiB read with that limit (413 past it); unknown fields ignored; a
  malformed body 400 `invalid_argument`. Each field is checked and
  normalized: `file` at most 512 bytes with no control characters;
  `version` an integer from 1 to the artifact's `current_version` (else
  the view is not counted); `referrer` a lowercase host name of at most
  253 bytes of letters, digits, `-` and `.` (anything else, the site's own
  host, the public origin's host and any site host under the site domain
  become `""`, "no referring site"); `load_ms` and `lcp_ms` integers from
  0 to 600 000 or null. A field of the wrong type, or a missing `file` or
  `version`, is 400; a `file` that fails its check or a `version` below 1
  is not counted (204); a timing outside its range is dropped and the view
  counted without it.
- **Rate:** `Attempt::PageView` in `clax_http::hosted` (`Limits`), per
  client address (`client_key`: an IPv6 address by its /64): 30 at once,
  one back every 2 s. A view over it is answered 204 and not counted: a
  429 tells a forger where the line is and a real viewer nothing useful.
  The limits live in memory and are per instance (qualifier `434e02d6` on
  `crates/clax-http/src/hosted.rs`), which is enough to bound one client.
- **Answer:** 204 with no body, counted or not.

Forging, plainly: a page cannot count for another clax (origin and host),
and a page counting extra views for its own clax is accepted. A script
outside a browser can set any `Origin` and claim to be a visitor of a site
open to the world, as with any public counter; the per-address rate, the
daily uniqueness of visitors and the caps on distinct values (below) bound
what it can do to one site's numbers, and it cannot reach a private site's
without a site session.

### What the server adds

From the request, in `clax_http::analytics` (classification in
`clax_core::analytics`, pure):

- **Country:** two letters, upper case, or `XX` for unknown. On the
  Worker, from `request.cf.country`: the Worker reads it (`worker`
  0.8.7's `Request::cf()`, `Cf::country()`) and sets `CF-IPCountry` on
  the request it forwards to the instance's object, replacing any value
  the client sent; the object's request has no `cf` of its own. The
  incoming request's headers are immutable in a Worker, so for this path
  the Worker builds the forwarded request anew, as `forward_undeclared`
  does. On a daemon with `[hosted]`, `CF-IPCountry` is trusted only when
  `behind_cloudflare` is set, as `CF-Connecting-IP` is (`Hosted::client`);
  otherwise the country is `XX`. Cloudflare's `T1` (Tor) is kept as given.
- **Device:** `phone`, `tablet` or `desktop`, from `Sec-CH-UA-Mobile: ?1`
  (phone) and else the User-Agent: `iPad`, `Tablet`, or `Android` without
  `Mobile`, is a tablet; `Mobi`, `iPhone` or `Android` a phone; anything
  else a desktop. iPadOS Safari sends a Mac's User-Agent and counts as a
  desktop; accepted.
- **Referring host:** the report's, as checked above.
- **Path:** the report's `file`.
- **Version:** the report's `version`.
- **Framed or direct:** the report's `framed`.

### Unique visitors

A visitor of a site on a day is

```
visitor = HMAC-SHA-256(day_secret, artifact_id ‖ 0x00 ‖ client_key(ip) ‖ 0x00 ‖ user_agent)[..16]
```

- `day_secret` is 32 random bytes (`clax_core::secret::random_bytes`) made
  at the first view of a UTC day and kept in `analytics_day_secrets`
  until that day's rollover, which deletes it. The store, not memory,
  keeps it, so a Worker object evicted mid-day hashes a returning visitor
  the same way after it wakes.
- The HMAC is `hmac_sha256`, moved from `crates/clax-http/src/hosted.rs`
  (private there today) to `clax_core::secret`.
- The IP address and User-Agent are read from the request and dropped
  with it; neither is stored or logged.

**Exact daily uniques need a per-day set, and this design keeps one**:
`analytics_seen`, one row per visitor and page they viewed that day
(16-byte hash and path; never the address), dropped at the day's
rollover. A view whose `(visitor, "")` row is new is the visitor's first of
the day: it counts one visitor for the day and attributes that visitor to
its country, device, referring host and version. A view whose
`(visitor, file)` row is new counts one visitor for that page. So the
dimension tables' visitors mean: per page, each visitor once a day; per
country, device, referring site and version, by the visitor's first view of
the day. The set is held in memory too (the day's hashes, 16 bytes each),
and a hash missing from memory is looked up in the table before it is
taken as new, so an object woken mid-day counts no one twice.

Not taken, recorded as an alternative: a HyperLogLog sketch per artifact and
day (no per-visitor rows at all, about 1.6 % error at 1 KiB per sketch).
It cannot give per-page visitors without a sketch per page and day, and
the set above lives only a day. `revisit:` per-visitor rows within the day
are refused, or a day's set outgrows its cost.

### Storage

Migration 68, the next free number (67 is
`CONTENT_SECURITY_POLICY_MIGRATION`, the build's last, asserted by
`migrations::tests` with `assert_eq!(super::CONTENT_SECURITY_POLICY_MIGRATION,
67)`), appended to `MIGRATIONS` in `crates/clax-store/src/migrations.rs` as
`ANALYTICS_MIGRATION = CONTENT_SECURITY_POLICY_MIGRATION + 1`, with its
mark `Mark::Table("analytics_days")` appended to `MARKS_AFTER_APP` (18 to
19 entries) and the "this build's last" doc moved to it. The tests that
name the last number as a literal follow (the fresh-database test and the
`MIGRATIONS[first + n]` index in the sequence test; qualifier `673bc83f`
on that file lists the kinds). If another branch lands 68 first, this one
renumbers (AGENTS.md).

```sql
ALTER TABLE artifacts ADD COLUMN analytics TEXT NOT NULL DEFAULT 'on'
    CHECK (analytics IN ('on','off'));
-- Daily totals per artifact.
CREATE TABLE analytics_days (
    artifact_id TEXT NOT NULL,
    day TEXT NOT NULL,            -- UTC, YYYY-MM-DD
    views INTEGER NOT NULL DEFAULT 0,
    visitors INTEGER NOT NULL DEFAULT 0,
    framed INTEGER NOT NULL DEFAULT 0,   -- views in Clax's frame
    PRIMARY KEY (artifact_id, day)
) WITHOUT ROWID;
-- Daily totals per artifact and dimension value.
CREATE TABLE analytics_dims (
    artifact_id TEXT NOT NULL,
    day TEXT NOT NULL,
    dim TEXT NOT NULL,            -- path, referrer, country, device, version
    value TEXT NOT NULL,
    views INTEGER NOT NULL DEFAULT 0,
    visitors INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (artifact_id, day, dim, value)
) WITHOUT ROWID;
-- Load timing histograms per artifact and day.
CREATE TABLE analytics_timings (
    artifact_id TEXT NOT NULL,
    day TEXT NOT NULL,
    metric TEXT NOT NULL CHECK (metric IN ('load','lcp')),
    bucket INTEGER NOT NULL,
    n INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (artifact_id, day, metric, bucket)
) WITHOUT ROWID;
-- Today's secret, deleted at the day's rollover.
CREATE TABLE analytics_day_secrets (day TEXT PRIMARY KEY, secret BLOB NOT NULL);
-- Today's visitors, deleted at the day's rollover.
CREATE TABLE analytics_seen (
    day TEXT NOT NULL,
    visitor BLOB NOT NULL,        -- 16 bytes
    path TEXT NOT NULL,           -- '' for the visitor's own row
    PRIMARY KEY (day, visitor, path)
) WITHOUT ROWID;
```

- `dim` has no SQL `CHECK`: the dimensions are a Rust enum
  (`clax_core::analytics::Dim`), so events a page records later can be
  rows of the same table (`event:<name>`) without rebuilding it.
- A timing bucket is `floor(4 · log2(ms))` clamped to 0..=80 (quarter
  octaves, about 19 % wide); a range's median is read from the merged
  histogram and reported as its bucket's geometric middle, within about
  10 %. Exact medians would need every sample.
- Each artifact and day takes at most 500 distinct values of `path` and of
  `referrer` (countries, devices and versions are bounded by what they
  are); a value past the cap is counted as `(other)`. This bounds a
  forging page's growth of its own rows.
- The setting does not travel: a push to a public Clax, a fork, a clone
  and a bundle each start at `on`, the default, and none carries totals.
- A deleted artifact's rows are deleted in the store's
  `delete_artifact` transaction (`crates/clax-store/src/artifacts.rs`),
  not by a listener (qualifier `f84ab739` on
  `crates/clax-worker/src/instance.rs` is what a listener risks).

`crates/clax-store/src/analytics.rs` (a `Store` impl block, as
`site_sessions.rs`) holds every query: the setting (read, and set under an
`AuditCtx` as `artifact.update` with `fields.analytics`, as
`set_content_security_policy` does), the day secret, the seen set's
lookups, a batch's write, the rollover, the retention pass and the reads.
It takes its time from the store's clock (`self.now()`), so tests drive it
with `ManualClock` (`crate::test_util::store_with_clock`).

### The write path, and what it costs on the Worker

The hosted Worker runs the whole server in one Durable Object per instance
(`InstanceDo`, `crates/clax-worker/src/instance.rs`), its database on the
SQLite VFS over the object's storage (`crates/clax-worker/vfs`): one
connection, and each committed transaction writes every dirty page as one
chunk to the object's storage (`PageStore::commit`), each a row written
for Cloudflare's billing. A site's pages already reach the object (the
Worker serves only `/_clax/` static files on a site host itself,
`crates/clax-worker/src/lib.rs`, `Route::Site`), so a counted view adds
one Worker request and one object request to the one the page made.

Written one view at a time, a view would dirty a leaf page of each table
it touches (days, dims, timings, seen): five to eight chunk writes a view.
So views are batched:

- `clax_http::analytics::Analytics`, an `AppState` field, keeps the views
  not yet written as sums keyed by artifact, day and dimension value,
  beside the day's seen set in memory. It is always there and empty, and
  counts, sweeps and is due only while `AppState::hosted` is set (the
  testkit and the hosts set `hosted` after `AppState::assemble`).
- A batch is written in one store transaction when it is 60 s old, when it
  holds 1 000 views, or at the day's rollover: from the request that finds
  it due, and from `AppState::sweep`, whose due time `AppState::next_sweep_in`
  gains (the batch's age limit while one is pending). The native daemon's
  sweeper runs `sweep` every `SWEEP_INTERVAL` (5 s,
  `crates/clax-server/src/core.rs`); the Worker's alarm runs it when
  `next_sweep_in` says (`timed_work`), so an idle object still writes its
  batch and then sleeps.
- A busy instance then writes a few chunks a minute per counted artifact,
  however many views it served. A transaction stays within the VFS's cap
  (`Limits::max_transaction_bytes`); a batch too large for it is written
  in parts.
- What an eviction loses: the views of the last batch not yet written, at
  most 60 s of them, with their seen-set entries (so they are not
  half-counted). Recorded as a waiver on the batch's flush, beside the
  planner refresh's in-memory state (qualifier `425816ee`), which has the
  same shape.

### The day's rollover, and retention

Days are UTC days. At the first sweep after midnight UTC
(`next_sweep_in` names midnight while the day's set or secret exists), one
transaction writes the old day's pending batch, then deletes the old day's
secret and its `analytics_seen` rows. After it no hash of that day can be
recomputed or matched. The seen rows go in bounded passes if there are many
(the secret goes in the first), each within the transaction cap, the next
pass at the next sweep. The old day's seen rows still in the batch are
dropped rather than written and deleted. A process does not know at start
whether an earlier one left a day open, so `next_sweep_in` is due at once
until the first sweep has read the earliest stored day
(`Store::analytics_open_day`): on the Worker, one store read per wake, made
at the object's first event (`InstanceDo::revive`), so a wake sets no alarm
for that read alone.

Retention: rows of `analytics_days`, `analytics_dims` and
`analytics_timings` whose day is more than 13 calendar months before today
(`day < clax_core::analytics::oldest_kept(today)`: the same day of the month
13 months back, or that month's last day where it is shorter, as chrono's
`checked_sub_months` gives; SQLite's `date(today, '-13 months')` would
overflow 31 March into 3 March instead) are deleted in bounded passes, at each
rollover and in the existing prune passes (the native core's pruner,
`prune_wake`, and the Worker's `prune_wanted`), so an instance with no
views for a while still sheds its old rows at its next prune. Nothing else
schedules work: there is no general job runner (`JOBS_MIGRATION` is the
Clax app's agent jobs, not a scheduler).

`e2e-hosted.mjs` checks that the Worker's alarm writes a batch: with
`CLAX_TEST_ROUTES`, `CLAX_TEST_ANALYTICS_BATCH_MS` writes a batch that
many milliseconds old (`Analytics::with_max_age`), so the alarm after a
view (5 s on) writes it, seen through `GET /_worker/alarm?artifact=<ID>`'s
`views_pending` and through the read route after a simulated wake. The
rollover's Worker path is covered by the store's and the server's tests,
which call `sweep` with a moved clock; no test passes midnight under
`wrangler dev`.

### The setting

- Stored as `artifacts.analytics` (`on` or `off`), read by
  `Store::analytics_on` (a draft is never counted, so it has no setting of
  its own; a missing or deleted artifact reads as off).
- Read: on a hosted Clax, `GET /api/artifacts/<id>` carries
  `analytics: "on" | "off"` for a caller who administers the artifact
  (`clax_http::agent::administers`) and leaves it out for anyone else and
  on a local Clax. The shell offers the Analytics panel when it is there.
- Written: `PATCH /api/artifacts/<id> {"analytics": "off"}` (`"on"`
  turns it on again; anything else 400 `invalid_argument`), as a new
  `SettingChange::Analytics(bool)` in
  `crates/clax-server/src/routes/setting_change.rs`, which needs no
  approval (`SettingChange::approval` is `None`): whoever administers the artifact may send it,
  anyone else 403 `admin_only` (a person whose rights do not let them
  update the artifact, a viewer, is refused 403 `not_permitted` first, as
  for every `PATCH`); a page never may (403 `not_for_pages`); an
  agent's change asks no one; one setting changed per request, as today
  (the error text of the "one at a time" refusal names all three). The
  browser may send it alone without the token (`PatchBody::site_only`
  counts it). On a local Clax it is 400 `invalid_argument` ("a local Clax
  counts no visits"). A draft is refused as for the Content Security
  Policy (`app_drafts::refused`).
- Recorded as `artifact.update` with `fields.analytics` (unlike
  `site_access`, qualifier `91c1a803`, which is not recorded).
- Either change holds from the next request; pages a browser keeps
  revalidate to the new tag (the `ETag` names it).
- Turning it off stops counting and keeps what was counted, which the panel
  still shows until retention deletes it (Open question 3).

### The read API

`GET /api/artifacts/<id>/analytics?from=YYYY-MM-DD&to=YYYY-MM-DD` in
`crates/clax-server/src/routes/analytics.rs`:

- Hosted only: 404 `not_found` on a local Clax.
- Whoever administers the artifact, from their browser or with the owner's
  token; anyone else 403 `admin_only`. It refuses agents
  (`DeniedToGrants::refuse_agents`, 403 `not_for_agents`), as `GET
  /api/artifacts/{aid}/permissions` does (Open question 5), and pages (403
  `not_for_pages`, as that route does). Its handler takes `DeniedToGrants`,
  which makes it Denied: `clax_http::agent::ROUTE_NEEDS` lists only the
  routes agents reach (the route table's generator refuses a Denied entry
  there), so `agent.rs` does not change. The contract's route table lists it
  in its second table, generated from the router and the handler's source,
  once `routes/guards.rs`'s `module_source_opt` names the module; it is
  regenerated (`CLAX_UPDATE_GOLDEN=1` and the test
  `the_contract_route_table_is_the_declared_one`,
  `crates/clax-server/src/routes/guards.rs`).
- `from` and `to` default to the last 30 days ending today (UTC); the
  range is at most 400 days and within retention (400 `invalid_argument`
  otherwise).
- The answer (`Cache-Control: private, no-store`):

```json
{
  "analytics": "on",
  "from": "2026-10-21", "to": "2026-11-19",
  "totals": {"visitors": 412, "views": 980, "framed": 120,
             "load_ms_median": 840, "lcp_ms_median": 1210},
  "days": [{"day": "2026-10-21", "visitors": 9, "views": 20}, …],
  "versions": [{"n": 4, "published_at": "2026-11-02T14:03:11Z"}, …],
  "pages": [{"value": "index.html", "views": 700, "visitors": 380}, …],
  "referrers": [{"value": "news.example.com", "views": 90, "visitors": 70}, …],
  "countries": [{"value": "DE", "views": 300, "visitors": 120}, …],
  "devices": [{"value": "phone", "views": 500, "visitors": 230}, …],
  "versions_viewed": [{"value": "4", "views": 600, "visitors": 300}, …]
}
```

- `days` holds every day of the range, zeros included; `visitors` in
  `totals` is the sum of the days' visitors (a visitor on two days counts
  twice); `versions` are those published in the range, the chart's
  markers (`versions.created_at`); each table lists its top 50 rows by
  views, the rest summed into one `(other)` row; the medians are null
  without samples. Today's not-yet-written batch is merged in, so a view
  shows up at once.

### The Analytics panel

On a hosted Clax, for an administrator, the top bar's more menu
(`web/shell/src/ui/MoreMenu.svelte`) lists **Analytics…** beside
Permissions…, and opens a panel over the stage, loaded lazily as the
Permissions panel is (`web/shell/src/ui/more-menu.svelte.ts`: a loader and
an opener, `analytics` and `analyticsOpen`), so the `artifact` entry's
budget does not grow by more than the item and its opener. A panel rather
than a shell route: `/a/<id>/<anything>` names a published file
(`web/shell/src/route.ts`), and a route would reserve another file name as
`get` does (Open question 4).

The panel (`web/shell/src/ui/AnalyticsPanel.svelte`, words and formatting
in `analytics-model.ts`, the fetch in `analytics-api.ts`):

- A range picker: 7 days, 30 days (the default), 90 days, 13 months.
- Totals: Visitors ("counted once a day"), Page views, Median load time
  (and LCP where there are samples), with the share of views in Clax's
  frame in small text.
- A visits-per-day chart: one bar per day (visitors, with page views as a
  lighter bar behind), inline SVG, the theme's tokens for color in both
  modes, a dashed mark at each version's publish day labelled `v<n>`, and
  a text table of the same numbers for screen readers.
- Tables: Pages, Referring sites ("Direct or unknown" for `""`),
  Countries (names from `Intl.DisplayNames`, `XX` "Unknown"), Devices,
  Versions.
- The setting: a line saying whether Clax is counting visits and one
  button, "Stop counting visits" or "Count visits", which sends the PATCH
  at once (no approval, nothing to explain beyond the line: "Clax counts
  page views and daily visitors on this clax's site, without cookies or
  stored addresses. Visitors who send Global Privacy Control are not
  counted.").
- Empty: "No visits yet" with the setting; off and empty: the setting
  alone.

### Where an agent meets it

Nowhere new: no MCP tool reads analytics in this design, and the page
contract gains one sentence (below) so an agent knows its pages are
counted on a hosted Clax.

## Docs

- `docs/contract.md` is hand-written; only its route table is generated
  (above). It changes in:
  - the site's `/_clax/api/` list ("Security model", the bullet that
    begins "`/_clax/api/` on a site host is the page runtime's backend"):
    `POST analytics`, its rules and 204;
  - the `PATCH /api/artifacts/<id>` rules beside `site_access` and
    `content_security_policy`: `analytics`;
  - `GET /api/artifacts/<id>`'s fields: `analytics`;
  - the new read route, in prose beside its table row;
  - the page contract: "On a hosted Clax the bridge reports one page view
    per page load to the site's own `/_clax/api/analytics`; a page needs
    to do nothing." The app's copy follows (`scripts/sync-app-prompt.sh`,
    checked by `//apps/clax_app:prompt_sync`).
- `crates/clax-server/src/routes/site_api.rs`'s module table gains the
  route.
- `crates/clax-worker/README.md` and `crates/clax-worker/src/instance.rs`'s
  module docs (Alarm): the analytics batch and rollover among the timed
  work, and the country header.
- `docs/legal/privacy.md` is the owner's and their counsel's (Open question
  6). The facts it would need: what is counted and kept (above), that
  nothing identifies a visitor past the day, that GPC is honoured for
  analytics, and that the runtime's own cookies are unchanged. Today it
  says "We do not currently respond to browser do-not-track signals",
  which this feature makes untrue for analytics.
- `docs/environments.md`: no change. No var, secret or binding is added;
  `request.cf.country` needs no zone setting.

## Crate layering

No new crate and no new edge in `scripts/check-crate-layers.py`'s
`ALLOWED`:

- `clax-core`: `analytics.rs` (dimensions, device classification,
  referrer checks, bot list, buckets, day keys; pure, both targets) and
  `secret::hmac_sha256`.
- `clax-store`: `analytics.rs` and migration 68.
- `clax-http`: `analytics.rs` (the batch, the hash, rollover due times),
  `Attempt::PageView` in `hosted.rs`, the `AppState` field and its hooks
  in `state.rs`.
- `clax-server`: `routes/analytics.rs` (read route), the `analytics` case
  in `routes/site_api.rs`, `routes/setting_change.rs`,
  `routes/artifacts.rs`, `routes/content.rs`, `router.rs`, the pruner in
  `core.rs`.
- `clax-worker`: the country header in `lib.rs`, the prune hook in
  `instance.rs`.

## Tests

All timing through injected clocks: the store's `ManualClock`, the hosted
limits' clock (`Hosted::with_clock`), the bridge's `Clock` and the shell's
`clock.ts`; no sleeps, no wall-clock budgets (AGENTS.md, and the linters
it names). New `#[cfg(test)]` modules carry
`#[warn(clippy::disallowed_methods)]`.

- **Unit, `clax-core`:** device classes over a table of real User-Agents
  and client hints; bot names; referrer host checks; bucket bounds and the
  median of merged histograms; day keys around midnight and month ends.
- **Unit, `clax-store`:** the migration on a fresh database and on one at
  67 (and the adoption marks); a batch written, read back by day and
  dimension; caps to `(other)`; the seen set's first-of-day and per-page
  rules; the rollover deletes the secret and the seen rows and only the
  old day's; retention at 13 calendar months, bounded passes, `more`; the
  setting's audit record; deletion with the artifact.
- **Unit, `clax-http`:** the hash is stable within a day and differs across
  days and sites; the batch's due times (age, size, midnight) on a moved
  clock; a view missing from memory but in the table is not counted again.
- **Integration, `crates/clax-server/tests/api_site_analytics.rs`**, on a
  hosted test daemon (`clax-testkit`): a world visitor's view counts once
  and twice from two addresses; a private site's needs a site session
  (401 without); `Origin` of another site, `Origin: null` and none are 403;
  `Sec-GPC: 1`, a bot User-Agent, a version past current, a draft, a
  setting off and a local daemon each count nothing (204); 2 KiB + 1 is
  413; the rate counts nothing past it; the page's tag has `data-count`
  exactly when it should, and its `ETag` changes with the setting;
  `CF-IPCountry` is used only when trusted; the rollover on a moved clock.
- **Integration, `crates/clax-server/tests/api_analytics.rs`:** the PATCH
  (admin only, never a page, an agent asks no one, a local Clax refuses,
  audited) and the read route (admin only, agents refused, range rules,
  zeros, top 50 and `(other)`, today's batch merged, version markers).
- **Unit, bridge, `web/bridge/test/count.test.ts`:** reports once after
  `load` with the tag's file and version, the referrer's host alone and the
  timings; nothing without `data-count`, with GPC, in the inspect and
  migration frames, while prerendering (then once on activation), on a
  back/forward restore; uses the captured `fetch` with `keepalive` and the
  JSON content type. The bundle gate (`//web:bundle_size`) measures the
  growth.
- **Unit, shell:** `analytics-model.test.ts` (formatting, country names,
  empty states), `analytics-panel.test.ts` (as `permissions-panel.test.ts`:
  a stubbed `fetch`, the range picker, the setting's PATCH, the chart's
  markers and its text table), the more menu's item present only with
  `analytics` in the artifact.
- **End to end, `web/e2e/analytics.spec.ts`** on a hosted daemon
  (`web/e2e/hosted-helpers.ts`): the owner opens a site to the world; a
  fresh browser context opens it directly and the owner's Analytics panel
  then shows one visitor, one page view, `index.html`, a direct view; a
  context with `Sec-GPC: 1` and `navigator.globalPrivacyControl` adds
  nothing; an invited person who does not administer it sees no
  Analytics… and is refused the route; after "Stop counting visits" a new
  visit adds nothing. On a local daemon a page's tag has no `data-count`
  and the page makes no request to `/_clax/api/analytics`.
- **Worker, `crates/clax-worker/e2e-hosted.mjs`:** a visit to a site under
  `wrangler dev --local` is counted with its country (workerd's `cf`, never
  the `CF-IPCountry` the visitor sent), the alarm writes the batch, and the
  read route answers it after a simulated wake. **Unit, `clax-worker`:**
  the forwarded country, and the alarm's prune deleting daily totals past
  their keep.

## Open questions

1. **The eager bridge's budget.** Recommended: the report in the eager
   bridge; if its measured growth passes the ~380 bytes of headroom, raise
   `bridge` to the measured size and record the raise. The alternative is
   a lazy part (no eager growth, one more request on a first load).
2. **Counting the administrator's own views.** Recommended: count every
   view. Excluding them works only when they arrive with a site session
   (a world-open site visited directly has none), so the numbers would
   depend on how the maker opened their page.
3. **Turning analytics off.** Recommended: stop counting and keep what was
   counted until retention deletes it; no "delete analytics" control in
   this design.
4. **Panel or page.** Recommended: a panel over the stage from the more
   menu, as Permissions. A shell route (`/a/<id>/analytics`) would give a
   link to share but reserve a file name.
5. **Agents.** Recommended: no agent reads analytics in this design (the
   route refuses agents, no MCP tool). Opening it to the administrator's
   agents later is one route class and one tool.
6. **The Privacy Policy.** The owner's and counsel's text; the facts are
   under "Docs". Recommended: changed in the same release as the feature.
7. **Days in UTC.** Recommended: UTC everywhere, the panel saying so. A
   per-artifact time zone would change the rollover and the day keys.

## Tasks

Each task builds and tests on its own (`just test-affected`), and is one
commit or a few. Thread IDs of records the work writes or closes go in its
commits.

**Task 1. Store and core: schema, setting, writes, reads, rollover,
retention.** Files: `crates/clax-store/src/migrations.rs` (68 and its
mark), `crates/clax-store/src/analytics.rs` (new), `crates/clax-store/src/lib.rs`
(module), `crates/clax-store/src/artifacts.rs` (deletion),
`crates/clax-core/src/analytics.rs` (new), `crates/clax-core/src/lib.rs`,
`crates/clax-core/src/secret.rs` (`hmac_sha256`, moved; its caller in
`crates/clax-http/src/hosted.rs` updated). Tests: the store and core unit
tests above, the migration tests renumbered. Depends on nothing.

**Task 2. Bridge: the report.** Files: `web/bridge/src/count.ts` (new),
`web/bridge/src/bridge.ts` (call it after `window.claude` is installed,
before the `framed` return), `web/bridge/test/count.test.ts` (new),
`web/perf/bundle-budget.json` (only if Open question 1 says raise). Tests:
the bridge unit tests, `//web:bundle_size`. Depends on nothing (the
request shape is this spec's); runs in parallel with Task 1.

**Task 3. Server: counting.** Files: `crates/clax-http/src/analytics.rs`
(new), `crates/clax-http/src/lib.rs`, `crates/clax-http/src/state.rs`
(field, `sweep`, `next_sweep_in`), `crates/clax-http/src/hosted.rs`
(`Attempt::PageView`), `crates/clax-server/src/routes/site_api.rs`
(the `analytics` case and module table), `crates/clax-core/src/wrap.rs`
(the attribute helper), `crates/clax-server/src/routes/content.rs`
(`data-count`, the `ETag`), `crates/clax-server/src/core.rs` (retention in
the pruner), `crates/clax-server/tests/api_site_analytics.rs` (new).
Depends on Task 1. Runs in parallel with Task 4 (no shared file but
`crates/clax-server/src/routes/mod.rs`, one line each).

**Task 4. Server: the setting and the read API.** Files:
`crates/clax-server/src/routes/setting_change.rs`
(`SettingChange::Analytics`), `crates/clax-server/src/routes/artifacts.rs`
(`PatchBody.analytics`, the GET field), `crates/clax-server/src/routes/analytics.rs`
(new), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/router.rs`,
`crates/clax-server/src/routes/guards.rs` (the module's source, for the
route table), `crates/clax-store/src/toolpath/redact.rs` (`analytics` as `Class::Safe`,
beside `content_security_policy`), `docs/contract.md` (the regenerated
route table only), `crates/clax-server/tests/api_analytics.rs` (new, a
module of `tests/integration.rs`).
Depends on Task 1. In parallel with Task 3.

**Task 5. Worker.** Files: `crates/clax-worker/src/lib.rs` (`CF-IPCountry`
from `request.cf` on the analytics path, replacing the client's),
`crates/clax-worker/src/instance.rs` (retention beside `prune_wanted`'s
pass; the open day read at the first event; `CLAX_TEST_ANALYTICS_BATCH_MS`
and `/_worker/alarm`'s `views_pending`; module docs),
`crates/clax-http/src/analytics.rs` (`Analytics::with_max_age`, for that
test var), `crates/clax-worker/README.md`,
`crates/clax-worker/e2e-hosted.mjs`. Tests: the crate's unit test of the
header rewrite, `just worker-test`. Depends on Task 3.

**Task 6. Shell: the Analytics panel.** Files:
`web/shell/src/ui/AnalyticsPanel.svelte`, `analytics-model.ts`,
`analytics-api.ts`, `analytics-model.test.ts`, `analytics-panel.test.ts`
(all new, under `web/shell/src/ui/`), `web/shell/src/ui/more-menu.svelte.ts`,
`web/shell/src/ui/MoreMenu.svelte`, `web/shell/src/ui/TopbarIsland.svelte`,
`web/scripts/bundle-size.mjs` and `web/perf/bundle-budget.json` (an
`analytics` chunk budget set by hand, as `people` and `queues`). Tests: the
shell unit tests, `//web:bundle_size` (the `artifact` entry stays within a
few bytes). Depends on Task 4's answer shape; can start against fixtures
while Tasks 3 to 5 run, as it touches none of their files (Task 2 touches
`web/perf/bundle-budget.json` only if it raises `bridge`: take that line
first).

**Task 7. End to end and docs.** Files: `web/e2e/analytics.spec.ts` (new),
`docs/contract.md` (the prose listed under "Docs"),
`apps/clax_app/assets/prompt/page-contract.md` (by
`scripts/sync-app-prompt.sh`), and the privacy text if Open question 6 is
answered. Tests: the e2e spec, `//apps/clax_app:prompt_sync`, then `just
check` once for the branch. Depends on Tasks 2, 3, 4 and 6 (and 5 for the
Worker's part of the docs).

Order: 1 and 2 together; then 3 and 4 together; 5 after 3; 6 after 4
(started earlier on fixtures); 7 last.
