import { describe, expect, it } from "vitest";
import { COUNT_PATH, type CountWorld, count } from "../src/count";
import { INSPECT_FRAME, MIGRATE_FRAME } from "../src/protocol";

type Sent = { url: string; init: RequestInit };

/** A page's world with hand-driven events and timers. */
function world(over: Partial<CountWorld> = {}, opts: { readyState?: DocumentReadyState; prerendering?: boolean; lcp?: number[]; nav?: object | null; failing?: boolean; referrer?: string } = {}) {
  const sent: Sent[] = [];
  const listeners: Record<string, (() => void)[]> = {};
  const on = (type: string, f: () => void) => { (listeners[type] ??= []).push(f); };
  const fire = (type: string) => { for (const f of listeners[type]?.splice(0) ?? []) f(); };
  const timers: (() => void)[] = [];
  const runTimers = () => { for (const f of timers.splice(0)) f(); };
  const doc = { readyState: opts.readyState ?? "loading", referrer: opts.referrer ?? "https://news.example.com/a/b?q=1", prerendering: opts.prerendering, addEventListener: on };
  let observed: unknown = null;
  let disconnected = false;
  class Observer {
    static supportedEntryTypes = opts.lcp ? ["largest-contentful-paint"] : ["navigation"];
    constructor(private cb: (l: { getEntries(): { startTime: number }[] }) => void) {}
    observe(o: unknown) { observed = o; this.cb({ getEntries: () => (opts.lcp ?? []).map(startTime => ({ startTime })) }); }
    disconnect() { disconnected = true; }
  }
  const navEntry = opts.nav === undefined ? { loadEventStart: 839.6 } : opts.nav;
  const w: CountWorld = {
    data: { count: "1" },
    meta: { artifact: "7q3k9mzx2b4t", version: 3, contract: "0.2.61", file: "guide/index.html" },
    framed: false,
    name: "",
    origin: "https://abc.sites.example",
    win: { addEventListener: on } as unknown as CountWorld["win"],
    nav: {},
    doc: doc as unknown as CountWorld["doc"],
    perf: { getEntriesByType: (t: string) => (t === "navigation" && navEntry ? [navEntry] : []) } as unknown as CountWorld["perf"],
    Observer: Observer as unknown as typeof PerformanceObserver,
    fetch: (async (url: string, init: RequestInit) => { sent.push({ url, init }); if (opts.failing) throw new TypeError("offline"); return new Response(null, { status: 204 }); }) as unknown as typeof fetch,
    clock: { setTimeout: (f: () => void) => { timers.push(f); return timers.length; } },
    ...over,
  };
  count(w);
  const body = (i = 0) => JSON.parse(sent[i].init.body as string);
  return { sent, fire, runTimers, doc, body, observed: () => observed, disconnected: () => disconnected };
}

describe("the page-view report", () => {
  it("is sent once, in the task after load, with the tag's file and version, the referrer's host and the timings", () => {
    const p = world({}, { lcp: [400.2, 1210.4] });
    p.runTimers();
    expect(p.sent).toEqual([]);
    p.fire("load");
    expect(p.sent).toEqual([]);
    p.runTimers();
    expect(p.sent).toHaveLength(1);
    expect(p.sent[0].url).toBe("https://abc.sites.example" + COUNT_PATH);
    expect(p.sent[0].init).toMatchObject({ method: "POST", keepalive: true, headers: { "content-type": "application/json" } });
    expect(p.body()).toEqual({ file: "guide/index.html", version: 3, framed: false, referrer: "news.example.com", load_ms: 840, lcp_ms: 1210 });
    expect(p.observed()).toEqual({ type: "largest-contentful-paint", buffered: true });
    expect(p.disconnected()).toBe(true);
    // A back/forward restore is no load: nothing more is sent.
    p.fire("pageshow"); p.fire("load"); p.runTimers();
    expect(p.sent).toHaveLength(1);
  });

  it("is sent at once when the bridge runs after load", () => {
    const p = world({ framed: true }, { readyState: "complete" });
    p.runTimers();
    expect(p.sent).toHaveLength(1);
    expect(p.body()).toMatchObject({ framed: true });
  });

  it("sends null timings without a navigation entry or LCP, and an empty referrer without one", () => {
    const p = world({}, { readyState: "complete", nav: null, referrer: "" });
    p.runTimers();
    expect(p.body()).toMatchObject({ referrer: "", load_ms: null, lcp_ms: null });
  });

  it("sends nothing without the tag's marker, with Global Privacy Control, or in the inspect and migration frames", () => {
    for (const over of [{ data: {} }, { data: undefined }, { data: { count: "0" } }, { nav: { globalPrivacyControl: true } },
      { framed: true, name: INSPECT_FRAME }, { framed: true, name: MIGRATE_FRAME }] as Partial<CountWorld>[]) {
      const p = world(over, { readyState: "complete" });
      p.fire("load"); p.runTimers();
      expect(p.sent, JSON.stringify(over)).toEqual([]);
    }
  });

  it("waits while prerendering and is sent once on activation, its timings from activation", () => {
    const p = world({}, { prerendering: true, lcp: [300], nav: { loadEventStart: 500, activationStart: 2000 } });
    p.fire("load"); p.runTimers();
    expect(p.sent).toEqual([]);
    p.doc.readyState = "complete";
    p.fire("prerenderingchange"); p.runTimers();
    expect(p.sent).toHaveLength(1);
    expect(p.body()).toMatchObject({ load_ms: 0, lcp_ms: 0 });
  });

  it("ignores a failed send", async () => {
    const p = world({}, { readyState: "complete", failing: true });
    p.runTimers();
    await Promise.resolve();
    expect(p.sent).toHaveLength(1);
  });
});
