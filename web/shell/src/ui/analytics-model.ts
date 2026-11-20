// The Analytics panel's words, formatting and chart geometry (spec
// 2026-11-19-site-analytics-design, "The Analytics panel"): the answer of
// `GET /api/artifacts/<id>/analytics`, its ranges, the totals' and tables'
// labels, the visits-per-day chart's bars and version marks, and the
// setting's line and button. Pure: AnalyticsPanel renders it. Days are UTC
// days, as the daemon counts them; "visitors" over a range is a sum of
// daily visitors, and every label says so.

export type AnalyticsSetting = "on" | "off";
/** One value of a dimension: a page, a referring host, a country, a device or a version. */
export type AnalyticsRow = { value: string; views: number; visitors: number };
export type AnalyticsDay = { day: string; visitors: number; views: number; framed?: number };
/** `GET /api/artifacts/<id>/analytics`. */
export type AnalyticsAnswer = {
  analytics: AnalyticsSetting;
  from: string;
  to: string;
  totals: { visitors: number; views: number; framed: number; load_ms_median: number | null; lcp_ms_median: number | null };
  days: AnalyticsDay[];
  versions: { n: number; published_at: string }[];
  pages: AnalyticsRow[];
  referrers: AnalyticsRow[];
  countries: AnalyticsRow[];
  devices: AnalyticsRow[];
  versions_viewed: AnalyticsRow[];
};

/** The value the daemon sums a table's rows past its top 50 into. */
export const OTHER = "(other)";

/** The range picker's choices; `30d` is the default. */
export type RangeKey = "7d" | "30d" | "90d" | "13m";
export const RANGES: readonly { key: RangeKey; label: string }[] = [
  { key: "7d", label: "7 days" },
  { key: "30d", label: "30 days" },
  { key: "90d", label: "90 days" },
  { key: "13m", label: "13 months" },
];
export const DEFAULT_RANGE: RangeKey = "30d";

const DAY_MS = 86_400_000;
const parse = (day: string) => Date.UTC(+day.slice(0, 4), +day.slice(5, 7) - 1, +day.slice(8, 10));
const iso = (ms: number) => new Date(ms).toISOString().slice(0, 10);
/** `day` moved `n` days (UTC). */
export const addDays = (day: string, n: number): string => iso(parse(day) + n * DAY_MS);
/** The UTC day of `ms`, `YYYY-MM-DD`. */
export const utcDay = (ms: number): string => iso(ms);

/** The oldest day the daemon keeps on `today`: the same day 13 months back,
 * or that month's last day where it is shorter (`clax_core::analytics::oldest_kept`). */
export function oldestKept(today: string): string {
  const y = +today.slice(0, 4), m = +today.slice(5, 7) - 1 - 13, d = +today.slice(8, 10);
  const last = new Date(Date.UTC(y, m + 1, 0)).getUTCDate();
  return iso(Date.UTC(y, m, Math.min(d, last)));
}

/** The first day `range` asks for on `today` (the daemon's last day is its
 * own today). `13m` starts a day after the oldest day kept, so a clock a day
 * behind the daemon's never asks for a day it no longer keeps. */
export function rangeFrom(range: RangeKey, today: string): string {
  switch (range) {
    case "7d": return addDays(today, -6);
    case "30d": return addDays(today, -29);
    case "90d": return addDays(today, -89);
    case "13m": return addDays(oldestKept(today), 1);
  }
}

const count = new Intl.NumberFormat("en");
/** A count as the panel shows it: `1,234`. */
export const formatCount = (n: number): string => count.format(n);

/** A median time: `840 ms`, `1.2 s`, `14 s`; null (no samples) as `NO_TIMING`. */
export function formatMs(ms: number | null): string {
  if (ms === null) return NO_TIMING;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const s = ms / 1000;
  return `${s < 10 ? s.toFixed(1) : Math.round(s)} s`;
}
export const NO_TIMING = "Not measured yet";

const short = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const long = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
/** A UTC day as the panel names it: `Nov 2`, or `Nov 2, 2026` with `year`. */
export const dayLabel = (day: string, year = false): string => (year ? long : short).format(parse(day));

/** Whether a range's days are named with their year: one longer than 90 days. */
export const withYear = (a: Pick<AnalyticsAnswer, "from" | "to">): boolean => (parse(a.to) - parse(a.from)) / DAY_MS + 1 > 90;

// Words.
export const TITLE = (title: string) => `Analytics for “${title}”`;
export const RANGE_LABEL = "Range";
export const VISITORS = "Daily visitors";
export const VISITORS_NOTE = "Counted once a day: someone who visits on three days counts three times.";
export const VIEWS = "Page views";
export const LOAD = "Median load time";
export const LCP = "Median time to main content";
export const DAYS_NOTE = "Days are in UTC.";
export const CHART = "Visits per day";
export const LOADING = "Loading…";
export const LOAD_FAILED = "Could not load the analytics";
export const CHANGE_FAILED = "Could not change whether Clax counts visits";
export const SETTING_HEADING = "Counting visits";
/** The setting's line while Clax counts. */
export const ON_NOTE = "Clax is counting visits to this clax's site: page views and daily visitors, without cookies or stored addresses. Visitors who send Global Privacy Control are not counted.";
/** The setting's line while it does not. */
export const OFF_NOTE = "Clax is not counting visits to this clax's site. What it counted before is kept.";
export const STOP = "Stop counting visits";
export const START = "Count visits";

/** The setting's line and button label for `setting`. */
export const settingWords = (setting: AnalyticsSetting): { note: string; button: string } =>
  setting === "on" ? { note: ON_NOTE, button: STOP } : { note: OFF_NOTE, button: START };

/** What the panel says when the range holds no view, or null when it holds
 * some. While counting is off and nothing was counted the setting alone
 * speaks (`""`). */
export function emptyText(a: AnalyticsAnswer, range: RangeKey): string | null {
  if (a.totals.views > 0) return null;
  if (a.analytics === "off") return "";
  return range === "13m" ? "No visits yet." : `No visits in the last ${RANGES.find(r => r.key === range)!.label}.`;
}

/** The share of views made in Clax's own frame, as a line, or null without views. */
export function framedText(t: AnalyticsAnswer["totals"]): string | null {
  if (t.views <= 0) return null;
  const pct = Math.round((t.framed / t.views) * 100);
  return `${pct}% of page views were inside Clax.`;
}

// Tables.
export type TableKind = "pages" | "referrers" | "countries" | "devices" | "versions_viewed";
export const TABLES: readonly { kind: TableKind; heading: string; column: string; other: string }[] = [
  { kind: "pages", heading: "Pages", column: "Page", other: "Other pages" },
  { kind: "referrers", heading: "Referring sites", column: "Site", other: "Other sites" },
  { kind: "countries", heading: "Countries", column: "Country", other: "Other countries" },
  { kind: "devices", heading: "Devices", column: "Device", other: "Other devices" },
  { kind: "versions_viewed", heading: "Versions", column: "Version", other: "Other versions" },
];
/** A table's visitors column: over a range, a sum of daily visitors, named as the total is. */
export const TABLE_VISITORS = VISITORS;
export const TABLE_VIEWS = "Views";
export const DIRECT = "Direct or unknown";
export const UNKNOWN_COUNTRY = "Unknown";
const DEVICES: Record<string, string> = { phone: "Phone", tablet: "Tablet", desktop: "Desktop" };

let regions: Intl.DisplayNames | null | undefined;
/** A country's name for its two-letter code: `XX` is unknown, `T1` Tor, and
 * a code `Intl.DisplayNames` does not know is shown as it is. */
export function countryName(code: string): string {
  if (code === "XX") return UNKNOWN_COUNTRY;
  if (code === "T1") return "Tor";
  if (regions === undefined) { try { regions = new Intl.DisplayNames(["en"], { type: "region" }); } catch { regions = null; } }
  try { return regions?.of(code) ?? code; } catch { return code; }
}

/** A table's row as the panel shows it: the value named for its kind,
 * `(other)` as the table's own words. */
export function rowLabel(kind: TableKind, value: string): string {
  if (value === OTHER) return TABLES.find(t => t.kind === kind)!.other;
  switch (kind) {
    case "referrers": return value === "" ? DIRECT : value;
    case "countries": return countryName(value);
    case "devices": return DEVICES[value] ?? value;
    case "versions_viewed": return /^\d+$/.test(value) ? `v${value}` : value;
    default: return value;
  }
}

// The chart. Geometry is in a 100 × 100 box the SVG stretches over its
// width (`preserveAspectRatio="none"`); labels are HTML placed by percent.

/** One day's bars: page views (behind) and daily visitors, heights in percent of the box. */
export type ChartBar = { day: string; visitors: number; views: number; x: number; w: number; hVisitors: number; hViews: number; title: string };
/** A version mark: its centre in percent of the width and its label, `v4` (`v3, v4` for one day). */
export type ChartMark = { day: string; x: number; label: string };
export type Chart = { max: number; bars: ChartBar[]; marks: ChartMark[] };

/** The smallest 1, 2 or 5 × 10ⁿ at least `n` (1 for nothing). */
export function niceMax(n: number): number {
  if (n <= 1) return 1;
  const p = 10 ** Math.floor(Math.log10(n));
  for (const m of [1, 2, 5, 10]) if (m * p >= n) return m * p;
  return 10 * p;
}

/** The text of one day, as its bar's tooltip and the chart's table say it. */
export const dayText = (d: AnalyticsDay, year = false): string =>
  `${dayLabel(d.day, year)}: ${formatCount(d.visitors)} daily ${d.visitors === 1 ? "visitor" : "visitors"}, ${formatCount(d.views)} page ${d.views === 1 ? "view" : "views"}`;

/** The chart of `a`: a bar per day, scaled to a round maximum of page
 * views, and a mark at each day a version in `a.versions` was published. */
export function chart(a: Pick<AnalyticsAnswer, "days" | "versions" | "from" | "to">): Chart {
  const n = a.days.length;
  const slot = n ? 100 / n : 100;
  const w = slot * (n > 120 ? 0.9 : 0.7);
  const max = niceMax(Math.max(0, ...a.days.map(d => d.views)));
  const year = withYear(a);
  const bars = a.days.map((d, i) => ({
    day: d.day, visitors: d.visitors, views: d.views,
    x: i * slot + (slot - w) / 2, w,
    hVisitors: (d.visitors / max) * 100, hViews: (d.views / max) * 100,
    title: dayText(d, year),
  }));
  const index = new Map(a.days.map((d, i) => [d.day, i]));
  const byDay = new Map<string, number[]>();
  for (const v of a.versions) {
    const day = v.published_at.slice(0, 10);
    if (index.has(day)) byDay.set(day, [...byDay.get(day) ?? [], v.n]);
  }
  const marks = [...byDay].map(([day, ns]) => ({ day, x: (index.get(day)! + 0.5) * slot, label: ns.map(v => `v${v}`).join(", ") }));
  return { max, bars, marks };
}

const esc = (t: string) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const num = (n: number) => +n.toFixed(3);
/** The chart `c` as SVG markup in a 100 × 100 box stretched over its width
 * (hidden from screen readers, which read the panel's table): per day a
 * group with its tooltip (`<title>`), a full-height hit area, the page
 * views' bar (`rect.views`) and the daily visitors' (`rect.visitors`) in
 * front; a dashed line per version mark (`line.mark`), the top grid line
 * and the baseline; then each mark's label (`span.an-mark`, `v4`), placed
 * by percent of the width over the box. The panel's CSS colours the classes
 * from the theme. */
export function chartSvg(c: Chart): string {
  const slot = c.bars.length ? 100 / c.bars.length : 100;
  const rect = (cls: string, x: number, h: number, w: number) => `<rect class="${cls}" x="${num(x)}" y="${num(100 - h)}" width="${num(w)}" height="${num(h)}"/>`;
  const bars = c.bars.map((b, i) =>
    `<g><title>${esc(b.title)}</title>${rect("hit", i * slot, 100, slot)}${b.views ? rect("views", b.x, b.hViews, b.w) : ""}${b.visitors ? rect("visitors", b.x, b.hVisitors, b.w) : ""}</g>`).join("");
  const marks = c.marks.map(m => `<line class="mark" x1="${num(m.x)}" y1="0" x2="${num(m.x)}" y2="100"/>`).join("");
  const labels = c.marks.map(m => `<span class="an-mark" aria-hidden="true" data-version-mark="${esc(m.label)}" style="left: ${num(m.x)}%">${esc(m.label)}</span>`).join("");
  return `<svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true"><line class="grid" x1="0" y1="0.5" x2="100" y2="0.5"/>${bars}${marks}<line class="base" x1="0" y1="100" x2="100" y2="100"/></svg>${labels}`;
}

/** The chart's text alternative's summary: the range, its totals and the versions published in it. */
export function chartSummary(a: AnalyticsAnswer): string {
  const year = withYear(a);
  const t = a.totals;
  const head = `${CHART}, ${dayLabel(a.from, year)} to ${dayLabel(a.to, year)}: ${formatCount(t.visitors)} daily ${t.visitors === 1 ? "visitor" : "visitors"}, ${formatCount(t.views)} page ${t.views === 1 ? "view" : "views"}.`;
  const marks = chart(a).marks;
  return marks.length ? `${head} Published: ${marks.map(m => `${m.label} on ${dayLabel(m.day, year)}`).join("; ")}.` : head;
}
