import { describe, expect, it } from "vitest";
import {
  type AnalyticsAnswer, DIRECT, NO_TIMING, OFF_NOTE, ON_NOTE, START, STOP,
  addDays, chart, chartSummary, chartSvg, countryName, dayLabel, emptyText, formatCount, formatMs, framedText, niceMax, oldestKept, rangeFrom, rowLabel, settingWords, withYear,
} from "./analytics-model";

const days = (from: string, n: number, f: (i: number) => [number, number] = () => [0, 0]) =>
  Array.from({ length: n }, (_, i) => { const [visitors, views] = f(i); return { day: addDays(from, i), visitors, views, framed: 0 }; });
const answer = (over: Partial<AnalyticsAnswer> = {}): AnalyticsAnswer => ({
  analytics: "on", from: "2026-11-13", to: "2026-11-19",
  totals: { visitors: 4, views: 5, framed: 1, load_ms_median: 800, lcp_ms_median: null },
  days: days("2026-11-13", 7, i => (i === 6 ? [4, 5] : [0, 0])),
  versions: [], pages: [], referrers: [], countries: [], devices: [], versions_viewed: [],
  ...over,
});

describe("ranges", () => {
  it("count back from today, both ends included", () => {
    expect(rangeFrom("7d", "2026-11-19")).toBe("2026-11-13");
    expect(rangeFrom("30d", "2026-11-19")).toBe("2026-10-21");
    expect(rangeFrom("90d", "2026-11-19")).toBe("2026-08-22");
  });
  it("13 months start a day after the oldest day the daemon keeps", () => {
    expect(oldestKept("2026-11-19")).toBe("2025-10-19");
    // A shorter month takes its last day, as chrono's checked_sub_months does.
    expect(oldestKept("2026-03-31")).toBe("2025-02-28");
    expect(oldestKept("2028-03-29")).toBe("2027-02-28");
    expect(rangeFrom("13m", "2026-11-19")).toBe("2025-10-20");
  });
  it("move across months and years", () => {
    expect(addDays("2026-01-01", -1)).toBe("2025-12-31");
    expect(addDays("2028-02-28", 1)).toBe("2028-02-29");
  });
});

describe("formatting", () => {
  it("counts with separators", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(12345)).toBe("12,345");
  });
  it("times in ms, then seconds", () => {
    expect(formatMs(840)).toBe("840 ms");
    expect(formatMs(1210)).toBe("1.2 s");
    expect(formatMs(14_400)).toBe("14 s");
    expect(formatMs(null)).toBe(NO_TIMING);
  });
  it("names UTC days, with the year for a range over 90 days", () => {
    expect(dayLabel("2026-11-02")).toBe("Nov 2");
    expect(dayLabel("2026-11-02", true)).toBe("Nov 2, 2026");
    expect(withYear({ from: "2026-08-22", to: "2026-11-19" })).toBe(false);
    expect(withYear({ from: "2026-08-21", to: "2026-11-19" })).toBe(true);
  });
  it("says the share of views inside Clax", () => {
    expect(framedText({ visitors: 4, views: 8, framed: 1, load_ms_median: null, lcp_ms_median: null })).toBe("13% of page views were inside Clax.");
    expect(framedText({ visitors: 0, views: 0, framed: 0, load_ms_median: null, lcp_ms_median: null })).toBeNull();
  });
});

describe("table rows", () => {
  it("name countries, unknown and Tor", () => {
    expect(countryName("DE")).toBe("Germany");
    expect(countryName("XX")).toBe("Unknown");
    expect(countryName("T1")).toBe("Tor");
    expect(rowLabel("countries", "US")).toBe("United States");
  });
  it("name the direct referrer, devices, versions and each table's (other) row", () => {
    expect(rowLabel("referrers", "")).toBe(DIRECT);
    expect(rowLabel("referrers", "news.example.com")).toBe("news.example.com");
    expect(rowLabel("devices", "phone")).toBe("Phone");
    expect(rowLabel("versions_viewed", "4")).toBe("v4");
    expect(rowLabel("pages", "index.html")).toBe("index.html");
    expect(rowLabel("pages", "(other)")).toBe("Other pages");
    expect(rowLabel("referrers", "(other)")).toBe("Other sites");
    expect(rowLabel("countries", "(other)")).toBe("Other countries");
    expect(rowLabel("devices", "(other)")).toBe("Other devices");
    expect(rowLabel("versions_viewed", "(other)")).toBe("Other versions");
  });
});

describe("empty states and the setting", () => {
  it("say there were no visits while counting, and nothing but the setting while off", () => {
    const none = { totals: { visitors: 0, views: 0, framed: 0, load_ms_median: null, lcp_ms_median: null } };
    expect(emptyText(answer(none), "13m")).toBe("No visits yet.");
    expect(emptyText(answer(none), "7d")).toBe("No visits in the last 7 days.");
    expect(emptyText(answer({ ...none, analytics: "off" }), "30d")).toBe("");
    expect(emptyText(answer(), "30d")).toBeNull();
    expect(emptyText(answer({ analytics: "off" }), "30d")).toBeNull();
  });
  it("words the setting's line and its button by what is held", () => {
    expect(settingWords("on")).toEqual({ note: ON_NOTE, button: STOP });
    expect(settingWords("off")).toEqual({ note: OFF_NOTE, button: START });
    expect(STOP).toBe("Stop counting visits");
    expect(START).toBe("Count visits");
  });
});

describe("the chart", () => {
  it("scales to a round maximum", () => {
    expect([0, 1, 3, 10, 11, 47, 120].map(niceMax)).toEqual([1, 1, 5, 10, 20, 50, 200]);
  });
  it("draws a bar per day with page views behind daily visitors", () => {
    const c = chart(answer({ days: days("2026-11-18", 2, i => (i ? [3, 10] : [0, 0])) }));
    expect(c.max).toBe(10);
    expect(c.bars.map(b => [b.x, b.w, b.hVisitors, b.hViews])).toEqual([[7.5, 35, 0, 0], [57.5, 35, 30, 100]]);
    expect(c.bars[1].title).toBe("Nov 19: 3 daily visitors, 10 page views");
    expect(c.bars[0].title).toBe("Nov 18: 0 daily visitors, 0 page views");
  });
  it("marks each day a version was published, one mark for two on a day, none outside the range", () => {
    const c = chart(answer({ versions: [
      { n: 2, published_at: "2026-11-14T08:00:00Z" },
      { n: 3, published_at: "2026-11-19T09:00:00Z" },
      { n: 4, published_at: "2026-11-19T10:00:00Z" },
      { n: 1, published_at: "2026-11-01T10:00:00Z" },
    ] }));
    expect(c.marks.map(m => [m.day, m.label])).toEqual([["2026-11-14", "v2"], ["2026-11-19", "v3, v4"]]);
    expect(c.marks[0].x).toBeCloseTo((1.5 / 7) * 100);
  });
  it("draws as SVG: a group per day with its tooltip and bars, a dashed line and a label per mark", () => {
    const svg = chartSvg(chart(answer({ days: days("2026-11-18", 2, i => (i ? [3, 10] : [0, 0])), versions: [{ n: 5, published_at: "2026-11-19T01:00:00Z" }] })));
    expect(svg).toBe('<svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true"><line class="grid" x1="0" y1="0.5" x2="100" y2="0.5"/>'
      + '<g><title>Nov 18: 0 daily visitors, 0 page views</title><rect class="hit" x="0" y="0" width="50" height="100"/></g>'
      + '<g><title>Nov 19: 3 daily visitors, 10 page views</title><rect class="hit" x="50" y="0" width="50" height="100"/><rect class="views" x="57.5" y="0" width="35" height="100"/><rect class="visitors" x="57.5" y="70" width="35" height="30"/></g>'
      + '<line class="mark" x1="75" y1="0" x2="75" y2="100"/><line class="base" x1="0" y1="100" x2="100" y2="100"/></svg>'
      + '<span class="an-mark" aria-hidden="true" data-version-mark="v5" style="left: 75%">v5</span>');
  });

  it("says itself in words", () => {
    expect(chartSummary(answer({ versions: [{ n: 2, published_at: "2026-11-14T08:00:00Z" }] })))
      .toBe("Visits per day, Nov 13 to Nov 19: 4 daily visitors, 5 page views. Published: v2 on Nov 14.");
    expect(chartSummary(answer({ totals: { visitors: 1, views: 1, framed: 0, load_ms_median: null, lcp_ms_median: null } })))
      .toBe("Visits per day, Nov 13 to Nov 19: 1 daily visitor, 1 page view.");
  });
});
