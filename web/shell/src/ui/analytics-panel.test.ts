import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { writable } from "svelte/store";
import AnalyticsPanel from "./AnalyticsPanel.svelte";
import { type AnalyticsAnswer, addDays } from "./analytics-model";
import { analyticsOpen } from "./more-menu.svelte";

const ID = "7q3k9mzx2b4t";
const TODAY = "2026-11-19";
const row = (value: string, views: number, visitors: number) => ({ value, views, visitors });
const answer = (from: string, over: Partial<AnalyticsAnswer> = {}): AnalyticsAnswer => {
  const n = Math.round((Date.parse(TODAY) - Date.parse(from)) / 86_400_000) + 1;
  return {
    analytics: "on", from, to: TODAY,
    totals: { visitors: 4, views: 5, framed: 1, load_ms_median: 800, lcp_ms_median: null },
    days: Array.from({ length: n }, (_, i) => ({ day: addDays(from, i), visitors: i === n - 1 ? 4 : 0, views: i === n - 1 ? 5 : 0, framed: 0 })),
    versions: [{ n: 2, published_at: `${TODAY}T10:00:00Z` }],
    pages: [row("index.html", 4, 3), row("(other)", 1, 1)],
    referrers: [row("", 3, 3), row("news.example.com", 2, 1)],
    countries: [row("DE", 3, 2), row("XX", 2, 2)],
    devices: [row("phone", 5, 4)],
    versions_viewed: [row("2", 5, 4)],
    ...over,
  };
};

const mounted: object[] = [];
type Calls = { method: string; url: string; body?: unknown }[];
function setup(reply: (from: string) => AnalyticsAnswer, patch?: (body: { analytics: string }) => Response) {
  const calls: Calls = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
    calls.push({ method: init.method ?? "GET", url, body });
    if (init.method === "PATCH") return patch?.(body) ?? new Response(JSON.stringify({ artifact: { analytics: body.analytics } }), { status: 200 });
    const from = new URL(url, location.href).searchParams.get("from")!;
    return new Response(JSON.stringify(reply(from)), { status: 200 });
  }));
  const ctl = { id: ID, state: writable({ data: { artifact: { title: "Forecast", analytics: "on" } } }) };
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(AnalyticsPanel, { target, props: { ctl: ctl as never, today: () => TODAY } }));
  flushSync();
  return calls;
}
afterEach(() => { for (const m of mounted.splice(0)) unmount(m); document.body.replaceChildren(); vi.unstubAllGlobals(); });
const button = (name: string) => [...document.querySelectorAll("button")].find(b => b.textContent === name);
const q = (sel: string) => document.querySelector<HTMLElement>(sel);
const texts = (sel: string) => [...document.querySelectorAll(sel)].map(e => e.textContent?.trim());

describe("AnalyticsPanel", () => {
  it("opens on the last 30 days with the totals, the chart and its text, and a table per dimension with its (other) row", async () => {
    const calls = setup(from => answer(from));
    expect(analyticsOpen.current).not.toBeNull();
    analyticsOpen.current!();
    await vi.waitFor(() => expect(q("[data-testid=analytics-totals]")).not.toBeNull());
    expect(calls[0]).toEqual({ method: "GET", url: `/api/artifacts/${ID}/analytics?from=2026-10-21` });
    expect(q("#an-title")!.textContent).toBe("Analytics for “Forecast”");
    expect(q("[aria-pressed=true]")!.textContent).toBe("30 days");
    expect(texts("[data-testid=analytics-totals] dt")).toEqual(["Daily visitors", "Page views", "Median load time"]);
    expect(texts("[data-testid=analytics-totals] dd")).toEqual(["4", "5", "800 ms"]);
    expect(document.body.textContent).toContain("Counted once a day: someone who visits on three days counts three times.");
    expect(document.body.textContent).toContain("20% of page views were inside Clax.");
    // The chart: 30 bars, a mark at the version published today, and its text alternative.
    expect(document.querySelectorAll("[data-testid=analytics-chart] svg g").length).toBe(30);
    expect(texts("[data-version-mark]")).toEqual(["v2"]);
    expect(document.querySelectorAll("[data-testid=analytics-chart] svg line.mark").length).toBe(1);
    expect(q("[data-testid=analytics-chart] svg")!.getAttribute("aria-hidden")).toBe("true");
    expect(q("[data-testid=analytics-chart-summary]")!.textContent).toBe("Visits per day, Oct 21 to Nov 19: 4 daily visitors, 5 page views. Published: v2 on Nov 19.");
    const table = document.querySelectorAll("[data-testid=analytics-chart-table] tbody tr");
    expect(table.length).toBe(30);
    expect([...table[29].children].map(c => c.textContent)).toEqual(["Nov 19", "4", "5"]);
    expect(texts(".an-table h3")).toEqual(["Pages", "Referring sites", "Countries", "Devices", "Versions"]);
    expect(texts("[data-table=pages] tbody th")).toEqual(["index.html", "Other pages"]);
    expect(texts("[data-table=referrers] tbody th")).toEqual(["Direct or unknown", "news.example.com"]);
    expect(texts("[data-table=countries] tbody th")).toEqual(["Germany", "Unknown"]);
    expect(texts("[data-table=devices] tbody th")).toEqual(["Phone"]);
    expect(texts("[data-table=versions_viewed] tbody th")).toEqual(["v2"]);
    expect(texts("[data-table=pages] tbody tr:first-child td")).toEqual(["3", "4"]);
    // A table's visitors are a sum of daily visitors, named as the total is.
    expect(texts("[data-table=pages] thead th")).toEqual(["Page", "Daily visitors", "Views"]);
    expect(q("[data-testid=analytics-setting-note]")!.textContent).toContain("Clax is counting visits to this clax's site");
    expect(button("Stop counting visits")).toBeDefined();
  });

  it("asks again for the range chosen", async () => {
    const calls = setup(from => answer(from));
    analyticsOpen.current!();
    await vi.waitFor(() => expect(q("[data-testid=analytics-totals]")).not.toBeNull());
    flushSync(() => button("7 days")!.click());
    await vi.waitFor(() => expect(document.querySelectorAll("[data-testid=analytics-chart] svg g").length).toBe(7));
    expect(calls.at(-1)!.url).toBe(`/api/artifacts/${ID}/analytics?from=2026-11-13`);
    expect(q("[aria-pressed=true]")!.textContent).toBe("7 days");
    flushSync(() => button("13 months")!.click());
    await vi.waitFor(() => expect(calls.at(-1)!.url).toBe(`/api/artifacts/${ID}/analytics?from=2025-10-20`));
    // Over 90 days, days are named with their year.
    await vi.waitFor(() => expect(q("[data-testid=analytics-chart-summary]")!.textContent).toContain("Oct 20, 2025 to Nov 19, 2026"));
  });

  it("turns counting off and on again at once, with no confirmation", async () => {
    const calls = setup(from => answer(from));
    analyticsOpen.current!();
    await vi.waitFor(() => expect(button("Stop counting visits")).toBeDefined());
    flushSync(() => button("Stop counting visits")!.click());
    await vi.waitFor(() => expect(button("Count visits")).toBeDefined());
    expect(q("[data-testid=analytics-setting-note]")!.textContent).toBe("Clax is not counting visits to this clax's site. What it counted before is kept.");
    // What was counted is still shown.
    expect(q("[data-testid=analytics-totals]")).not.toBeNull();
    flushSync(() => button("Count visits")!.click());
    await vi.waitFor(() => expect(button("Stop counting visits")).toBeDefined());
    expect(calls.filter(c => c.method === "PATCH")).toEqual([
      { method: "PATCH", url: `/api/artifacts/${ID}`, body: { analytics: "off" } },
      { method: "PATCH", url: `/api/artifacts/${ID}`, body: { analytics: "on" } },
    ]);
  });

  it("says why a change did not take, and keeps the setting the daemon holds", async () => {
    const refused = () => new Response(JSON.stringify({ error: { code: "admin_only", message: "only those who administer the page change it" } }), { status: 403 });
    setup(from => answer(from), refused);
    analyticsOpen.current!();
    await vi.waitFor(() => expect(button("Stop counting visits")).toBeDefined());
    flushSync(() => button("Stop counting visits")!.click());
    await vi.waitFor(() => expect(q("[role=alert]")?.textContent).toBe("Could not change whether Clax counts visits: only those who administer the page change it"));
    expect(button("Stop counting visits")).toBeDefined();
  });

  it("says there are no visits yet, and while off and empty shows the setting alone", async () => {
    const none = { totals: { visitors: 0, views: 0, framed: 0, load_ms_median: null, lcp_ms_median: null }, versions: [], pages: [], referrers: [], countries: [], devices: [], versions_viewed: [] };
    let off = false;
    setup(from => { const a = answer(from, none); return { ...a, days: a.days.map(d => ({ ...d, visitors: 0, views: 0 })), analytics: off ? "off" : "on" }; });
    analyticsOpen.current!();
    await vi.waitFor(() => expect(q("[data-testid=analytics-empty]")?.textContent).toBe("No visits in the last 30 days."));
    expect(q("[data-testid=analytics-chart]")).toBeNull();
    expect(button("Stop counting visits")).toBeDefined();
    flushSync(() => button("13 months")!.click());
    await vi.waitFor(() => expect(q("[data-testid=analytics-empty]")?.textContent).toBe("No visits yet."));
    off = true;
    flushSync(() => button("90 days")!.click());
    await vi.waitFor(() => expect(button("Count visits")).toBeDefined());
    expect(q("[data-testid=analytics-empty]")).toBeNull();
    expect(q("[data-testid=analytics-totals]")).toBeNull();
    expect(q("[data-testid=analytics-setting-note]")!.textContent).toBe("Clax is not counting visits to this clax's site. What it counted before is kept.");
  });

  it("follows the setting's change on an empty range: the setting alone once off, the empty line once on again", async () => {
    const none = { totals: { visitors: 0, views: 0, framed: 0, load_ms_median: null, lcp_ms_median: null }, versions: [], pages: [], referrers: [], countries: [], devices: [], versions_viewed: [] };
    setup(from => { const a = answer(from, none); return { ...a, days: a.days.map(d => ({ ...d, visitors: 0, views: 0 })) }; });
    analyticsOpen.current!();
    await vi.waitFor(() => expect(q("[data-testid=analytics-empty]")?.textContent).toBe("No visits in the last 30 days."));
    flushSync(() => button("Stop counting visits")!.click());
    await vi.waitFor(() => expect(button("Count visits")).toBeDefined());
    expect(q("[data-testid=analytics-empty]")).toBeNull();
    flushSync(() => button("Count visits")!.click());
    await vi.waitFor(() => expect(button("Stop counting visits")).toBeDefined());
    expect(q("[data-testid=analytics-empty]")?.textContent).toBe("No visits in the last 30 days.");
  });

  it("closes on Escape and gives the more menu no opener once unmounted", async () => {
    setup(from => answer(from));
    analyticsOpen.current!();
    await vi.waitFor(() => expect(q("[role=dialog]")).not.toBeNull());
    q("[role=dialog]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(q("[role=dialog]")).toBeNull();
    for (const m of mounted.splice(0)) unmount(m);
    expect(analyticsOpen.current).toBeNull();
  });
});
