import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { MAX_URL } from "../messages";
import { startLoader } from "./loader-app";
import { BOOT, RUNNING } from "./presence";
import { ROUTE_MS } from "./routes";

/** Timers a test advances by hand. */
function manual() {
  let now = 0;
  let queue: { at: number; fn: () => void }[] = [];
  return {
    now: () => now,
    set: (fn: () => void, ms: number) => { const e = { at: now + ms, fn }; queue.push(e); return e; },
    clear: (h: unknown) => { queue = queue.filter(e => e !== h); },
    advance(ms: number) {
      now += ms;
      for (const e of queue.filter(q => q.at <= now)) { queue = queue.filter(x => x !== e); e.fn(); }
    },
  };
}
const flush = () => new Promise(r => setTimeout(r, 0));

let sent: { t: string; url: string | null }[];
let rt: { id: string | undefined; sendMessage(m: never): Promise<unknown> };
let timers: ReturnType<typeof manual>;
let g: Record<string, unknown>;
let stop: () => void;

beforeEach(() => {
  history.replaceState(null, "", "/app");
  sent = [];
  g = {};
  timers = manual();
  rt = { id: "test-extension", sendMessage: async m => { sent.push(m); return null; } };
  stop = startLoader({ win: window, runtime: rt, g, timers, now: timers.now });
});
afterEach(() => stop());

describe("the loader", () => {
  it("greets the worker with the page's URL", () => {
    expect(sent).toEqual([{ t: "hello", url: location.href }]);
  });

  it("ignores navigation events the page dispatches", () => {
    sent = [];
    history.replaceState(null, "", "#/forged");
    for (let i = 0; i < 500; i++) {
      dispatchEvent(new HashChangeEvent("hashchange"));
      dispatchEvent(new PopStateEvent("popstate"));
    }
    timers.advance(ROUTE_MS * 4);
    expect(sent).toEqual([]);
  });

  it("reports a storm of the browser's navigations once per ROUTE_MS, with the URL read from location, and not an unchanged one", async () => {
    sent = [];
    for (let i = 0; i < 50; i++) location.hash = `#/storm-${i}`;
    await flush();
    expect(sent).toEqual([]);
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "route", url: location.href }]);
    expect(location.hash).toBe("#/storm-49");
    // A trusted event whose URL did not change reports nothing.
    history.replaceState(null, "", location.href);
    location.hash = "#/storm-49";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toHaveLength(1);
    location.hash = "#/next";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "route", url: location.href.replace("#/next", "#/storm-49") }, { t: "route", url: location.href }]);
  });

  it("stays quiet while the overlay runs in the document, which reports the routes itself", async () => {
    sent = [];
    g[BOOT] = "a".repeat(32);
    g[RUNNING] = { alive: (b: unknown) => b === "a".repeat(32), stop: () => {} };
    location.hash = "#/with-overlay";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([]);
    // An overlay of an earlier load of the extension does not count.
    g[BOOT] = "b".repeat(32);
    location.hash = "#/stale-overlay";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "route", url: location.href }]);
  });

  it("sends an address over MAX_URL as null, never the address", async () => {
    sent = [];
    location.hash = `#/${"x".repeat(MAX_URL)}`;
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "route", url: null }]);
  });

  it("stops once its extension context is gone", async () => {
    rt.id = undefined;
    location.hash = "#/after-reload";
    await flush();
    timers.advance(ROUTE_MS);
    rt.id = "test-extension";
    location.hash = "#/later";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "hello", url: expect.stringContaining("/app") }]);
  });
});
