import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type OverlayEnv, type OverlayRuntime, ROUTE_MS, startOnce } from "./overlay-app";
import { hasOverlay, setBoot } from "../sw/origins";
import { MAX_WAIT_MS, QUIET_MS, Resolver, type Timers } from "./resolver";

type Listener = (m: unknown, sender: chrome.runtime.MessageSender) => void;
const WORKER = { id: "test-extension" } as chrome.runtime.MessageSender;
const A = "01J9AAAAAAAAAAAAAAAAAAAAAA";

/** Timers a test advances by hand. */
function manual() {
  let now = 0;
  let queue: { at: number; fn: () => void }[] = [];
  const frames: (() => void)[] = [];
  const t: Timers & { advance(ms: number): void; now(): number } = {
    now: () => now,
    set: (fn, ms) => { const e = { at: now + ms, fn }; queue.push(e); return e; },
    clear: h => { queue = queue.filter(e => e !== h); },
    frame: fn => { frames.push(fn); },
    advance(ms) {
      now += ms;
      for (const e of queue.filter(q => q.at <= now)) { queue = queue.filter(x => x !== e); e.fn(); }
      while (frames.length) frames.shift()!();
    },
  };
  return t;
}
const flush = () => new Promise(r => setTimeout(r, 0));
const records = () => new Promise<void>(r => queueMicrotask(r));
const thread = (id: string, selector: string, quote: string) => ({
  id, status: "open", anchor: { kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
  addressed_pending: false,
});
const state = (threads: unknown[], extra: Record<string, unknown> = {}) =>
  ({ t: "state", page: null, route: null, threads, commentMode: false, pending: false, ...extra });

let sent: { t: string; [k: string]: unknown }[];
let listeners: Listener[];
let ticks: { fn: () => void; ms: number }[];
let roots: ShadowRoot[];
let rt: { -readonly [K in keyof OverlayRuntime]: OverlayRuntime[K] };
let timers: ReturnType<typeof manual>;
let stop: (() => void) | undefined;
let global: Record<string, unknown>;

function env(): OverlayEnv {
  return {
    doc: document,
    runtime: rt,
    timers,
    every: (fn, ms) => { const e = { fn, ms }; ticks.push(e); return () => { ticks = ticks.filter(x => x !== e); }; },
    now: () => timers.now(),
  };
}
const tell = (m: unknown, sender = WORKER) => listeners.forEach(l => l(m, sender));
const tick = (ms: number) => ticks.filter(e => e.ms === ms).forEach(e => e.fn());
const hosts = () => [...document.querySelectorAll("clax-overlay")];
const pins = () => roots.flatMap(r => [...r.querySelectorAll("button.pin")]);

beforeEach(() => {
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<div id="app"><main><button id="save">Save</button></main></div>`;
  sent = []; listeners = []; ticks = []; roots = []; global = {};
  timers = manual();
  rt = {
    id: "test-extension",
    sendMessage: async m => { sent.push(m as never); return null; },
    getURL: p => `chrome-extension://test-extension/${p}`,
    onMessage: { addListener: l => listeners.push(l), removeListener: l => { listeners = listeners.filter(x => x !== l); } },
  };
  const attach = HTMLElement.prototype.attachShadow;
  vi.spyOn(HTMLElement.prototype, "attachShadow").mockImplementation(function (this: HTMLElement, init: ShadowRootInit) {
    const r = attach.call(this, init);
    roots.push(r);
    return r;
  });
  stop = startOnce(env(), global);
});
afterEach(() => { stop?.(); vi.restoreAllMocks(); });

describe("the overlay", () => {
  it("starts once per document, in closed shadow roots, and tells the worker its URL", () => {
    expect(startOnce(env(), global)).toBeUndefined();
    expect(HTMLElement.prototype.attachShadow).toHaveBeenCalledTimes(2);
    for (const [init] of vi.mocked(HTMLElement.prototype.attachShadow).mock.calls) expect(init.mode).toBe("closed");
    expect(hosts().every(h => h.shadowRoot === null)).toBe(true);
    expect(sent).toEqual([{ t: "route", url: location.href }]);
  });

  it("does not use the worker's per-document flag", () => {
    expect(Object.keys(global)).not.toContain("claxOverlayLoaded");
  });

  it("draws a numbered pin per found thread and leaves the page's DOM free of thread data", () => {
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(0);
    expect(pins().map(p => p.textContent)).toEqual(["1"]);
    expect(document.documentElement.outerHTML).not.toContain(A);
    for (const h of hosts()) expect(h.getAttributeNames().sort()).toEqual(h.hasAttribute("popover") ? ["popover", "style"] : h.hasAttribute("style") ? ["style"] : []);
    expect(sent.filter(m => m.t === "resolved")).toEqual([{ t: "resolved", results: [{ id: A, found: true, method: "selector", rect: { x: 0, y: 0, w: 0, h: 0 } }] }]);
  });

  it("takes messages only from the worker, and only valid ones", () => {
    tell(state([thread(A, "#save", "Save")]), { id: "test-extension", tab: { id: 1 } } as chrome.runtime.MessageSender);
    tell(state([thread(A, "#save", "Save")]), { id: "another-extension" } as chrome.runtime.MessageSender);
    tell({ ...state([thread(A, "#save", "Save")]), extra: 1 });
    timers.advance(0);
    expect(pins()).toHaveLength(0);
  });

  it("does not resolve again or re-send results for a state that changed nothing it shows", async () => {
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(0);
    tell(state([thread(A, "#save", "Save")], { pending: true }));
    timers.advance(0);
    expect(sent.filter(m => m.t === "resolved")).toHaveLength(1);
    document.querySelector("#save")!.remove();
    await flush();
    timers.advance(QUIET_MS);
    expect(sent.filter(m => m.t === "resolved").at(-1)).toEqual({ t: "resolved", results: [{ id: A, found: false, method: null, rect: null }] });
    expect(pins()).toHaveLength(0);
  });

  it("turns comment mode on and off as the worker says", () => {
    tell(state([], { commentMode: true }));
    expect(document.documentElement.style.cursor).toBe("crosshair");
    tell(state([], { commentMode: false }));
    expect(document.documentElement.style.cursor).toBe("");
  });

  it("re-adds its host once when the page removes it, then gives up and says so", async () => {
    const host = hosts()[0];
    host.remove();
    await flush();
    expect(host.isConnected).toBe(true);
    host.remove();
    await flush();
    host.remove();
    await flush();
    expect(host.isConnected).toBe(false);
    expect(sent.filter(m => m.t === "removed")).toHaveLength(1);
  });

  it("reports a same-document navigation from the browser, at most once per ROUTE_MS, and not an unchanged URL", async () => {
    sent = [];
    for (let i = 0; i < 20; i++) location.hash = `#/storm-${i}`;
    await flush();
    expect(sent).toEqual([]);
    timers.advance(ROUTE_MS);
    expect(sent).toEqual([{ t: "route", url: location.href }]);
    expect(location.hash).toBe("#/storm-19");
    dispatchEvent(new HashChangeEvent("hashchange"));
    timers.advance(ROUTE_MS);
    expect(sent).toHaveLength(1);
    location.hash = "#/next";
    await flush();
    timers.advance(ROUTE_MS);
    expect(sent.at(-1)).toEqual({ t: "route", url: location.href });
    expect(sent).toHaveLength(2);
  });

  it("ignores navigation events the page dispatches", () => {
    sent = [];
    history.replaceState(null, "", "#/forged");
    for (let i = 0; i < 50; i++) {
      dispatchEvent(new HashChangeEvent("hashchange"));
      dispatchEvent(new PopStateEvent("popstate"));
    }
    timers.advance(ROUTE_MS * 4);
    expect(sent).toEqual([]);
  });

  it("sends a snapshot once the DOM has settled while an address is pending, at most every 10 s", () => {
    tell(state([], { pending: true }));
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(0); // a resolution is due
    timers.advance(0);
    const q = sent.filter(m => m.t === "quiet");
    expect(q).toHaveLength(1);
    expect(q[0].snapshot).toContain("Save");
    expect(q[0].snapshot).not.toContain("clax-overlay");
    timers.advance(5000);
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(1);
    timers.advance(5000);
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(2);
    tell(state([], { pending: false }));
    timers.advance(60_000);
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(2);
  });

  it("names in a quiet snapshot the threads its state showed waiting for one", () => {
    const B = "01J9BBBBBBBBBBBBBBBBBBBBBB";
    const C = "01J9CCCCCCCCCCCCCCCCCCCCCC";
    const waiting = { addressed_pending: true };
    tell(state([
      { ...thread(A, "#save", "Save"), ...waiting },
      thread(B, "#save", "Save"),
      { ...thread(C, "#save", "Save"), ...waiting, status: "resolved" },
    ], { pending: true }));
    timers.advance(0);
    tick(1000);
    expect(sent.filter(m => m.t === "quiet").map(m => m.pending)).toEqual([[A]]);
  });

  it("still snapshots and re-resolves a page that never goes quiet", async () => {
    tell(state([thread(A, "#save", "Save")], { pending: true }));
    timers.advance(0);
    const quiet = () => sent.filter(m => m.t === "quiet").length;
    const before = quiet();
    timers.advance(10_000);
    const runs = vi.spyOn(Resolver.prototype, "run");
    const p = document.createElement("p");
    document.body.appendChild(p);
    for (let i = 0; i < 100; i++) {
      p.textContent = String(i);
      await records();
      timers.advance(50);
      if (i % 20 === 19) tick(1000);
    }
    expect(runs.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(quiet()).toBeGreaterThan(before);
    expect(MAX_WAIT_MS).toBe(1000);
  });

  it("pings the worker every 20 s", () => {
    tick(20_000);
    expect(sent.at(-1)).toEqual({ t: "ping" });
  });

  it("selects the thread the worker focuses", () => {
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(0);
    tell({ t: "focus", threadId: A });
    expect(pins()[0].classList.contains("sel")).toBe(true);
    tell({ t: "focus", threadId: null });
    expect(pins()[0].classList.contains("sel")).toBe(false);
  });
  it("sends its results again once when the worker asks, without a loop", async () => {
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(0);
    tell(state([thread(A, "#save", "Save")]));
    tell({ t: "resend" });
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(QUIET_MS);
    expect(sent.filter(m => m.t === "resolved")).toHaveLength(2);
  });

  it("sends the results after the resolution a resend arrives during", () => {
    tell(state([thread(A, "#save", "Save")]));
    tell({ t: "resend" });
    expect(sent.filter(m => m.t === "resolved")).toHaveLength(0);
    timers.advance(0);
    expect(sent.filter(m => m.t === "resolved")).toHaveLength(1);
  });

  it("repairs a host the page restyles or moves once, then gives up and says so", async () => {
    const host = hosts()[0] as HTMLElement;
    const style = host.getAttribute("style");
    host.style.display = "none";
    await records();
    expect(host.getAttribute("style")).toBe(style);
    expect(sent.filter(m => m.t === "removed")).toHaveLength(0);
    host.removeAttribute("popover");
    await records();
    expect(host.getAttribute("popover")).toBeNull();
    expect(sent.filter(m => m.t === "removed")).toHaveLength(1);
  });

  it("puts back a host the page moved", async () => {
    const host = hosts()[0];
    document.body.appendChild(host);
    await records();
    expect(host.parentNode).toBe(document.documentElement);
  });

  it("stops, removing everything it drew, when the extension context is gone", () => {
    rt.id = undefined;
    tick(20_000);
    expect(hosts()).toHaveLength(0);
    expect(listeners).toHaveLength(0);
    expect(global).not.toHaveProperty("claxOverlayStarted");
    stop = startOnce(env(), global);
    expect(stop).toBeTypeOf("function");
  });

  it("counts as present only while live and of the worker's load of the extension, and a new load's overlay replaces it", () => {
    stop?.();
    stop = undefined;
    const g = globalThis as Record<string, unknown>;
    const BOOT_A = "a".repeat(32), BOOT_B = "b".repeat(32);
    try {
      // The worker's probe and boot run in the isolated world: here, this global.
      setBoot(BOOT_A);
      const first = startOnce(env());
      expect(first).toBeTypeOf("function");
      expect(hasOverlay(BOOT_A)).toBe(true);
      const drawn = hosts().length;
      expect(startOnce(env())).toBeUndefined(); // injected twice, it starts once
      expect(hosts()).toHaveLength(drawn);
      // The extension was reloaded: the old overlay's mark stays in the world, but it is not this load's.
      expect(hasOverlay(BOOT_B)).toBe(false);
      setBoot(BOOT_B);
      const second = startOnce(env());
      expect(second).toBeTypeOf("function");
      expect(hosts()).toHaveLength(drawn); // the old one stopped and removed its hosts
      expect(hasOverlay(BOOT_B)).toBe(true);
      // Its extension context gone, it is no longer present either.
      rt.id = undefined;
      expect(hasOverlay(BOOT_B)).toBe(false);
      second!();
      first!();
      expect(hosts()).toHaveLength(0);
    } finally {
      delete g.claxOverlayStarted;
      delete g.claxBoot;
    }
  });

  it("scrolls to a found thread and flashes it", () => {
    const into = vi.fn();
    Object.defineProperty(Element.prototype, "scrollIntoView", { value: into, configurable: true });
    try {
      tell(state([thread(A, "#save", "Save")]));
      timers.advance(0);
      tell({ t: "scroll-to", threadId: A });
      expect(into).toHaveBeenCalledTimes(1);
      expect(into.mock.contexts[0]).toBe(document.querySelector("#save"));
      expect(pins()[0].classList.contains("sel")).toBe(true);
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it("measures once per frame however many scrolls arrive", () => {
    tell(state([thread(A, "#save", "Save")]));
    timers.advance(0);
    const measure = vi.spyOn(Resolver.prototype, "measure");
    for (let i = 0; i < 10; i++) document.dispatchEvent(new Event("scroll"));
    timers.advance(0);
    expect(measure).toHaveBeenCalledTimes(1);
  });

  it("ignores a close-composer for a pick it does not show", () => {
    tell(state([], { commentMode: false }));
    tell({ t: "close-composer", pickId: "0".repeat(32), posted: true });
    expect(document.documentElement.style.cursor).toBe("");
  });
});
