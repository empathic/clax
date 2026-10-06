import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type OverlayEnv, startOnce } from "./overlay-app";
import { QUIET_MS, type Timers } from "./resolver";

type Listener = (m: unknown, sender: chrome.runtime.MessageSender) => void;
const WORKER = { id: "test-extension" } as chrome.runtime.MessageSender;
const A = "01J9AAAAAAAAAAAAAAAAAAAAAA";

/** Timers a test advances by hand. */
function manual() {
  let now = 0;
  let queue: { at: number; fn: () => void }[] = [];
  const frames: (() => void)[] = [];
  const t: Timers & { advance(ms: number): void } = {
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
const thread = (id: string, selector: string, quote: string) => ({
  id, status: "open", anchor: { kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
});
const state = (threads: unknown[], extra: Record<string, unknown> = {}) =>
  ({ t: "state", page: null, route: null, threads, commentMode: false, pending: false, ...extra });

let sent: { t: string; [k: string]: unknown }[];
let listeners: Listener[];
let ticks: { fn: () => void; ms: number }[];
let roots: ShadowRoot[];
let clock: number;
let timers: ReturnType<typeof manual>;
let stop: (() => void) | undefined;
let global: Record<string, unknown>;

function env(): OverlayEnv {
  return {
    doc: document,
    runtime: {
      id: "test-extension",
      sendMessage: async m => { sent.push(m as never); return null; },
      getURL: p => `chrome-extension://test-extension/${p}`,
      onMessage: { addListener: l => listeners.push(l), removeListener: l => { listeners = listeners.filter(x => x !== l); } },
    },
    timers,
    every: (fn, ms) => { const e = { fn, ms }; ticks.push(e); return () => { ticks = ticks.filter(x => x !== e); }; },
    now: () => clock,
  };
}
const tell = (m: unknown, sender = WORKER) => listeners.forEach(l => l(m, sender));
const tick = (ms: number) => ticks.filter(e => e.ms === ms).forEach(e => e.fn());
const hosts = () => [...document.querySelectorAll("clax-overlay")];
const pins = () => roots.flatMap(r => [...r.querySelectorAll("button.pin")]);

beforeEach(() => {
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<div id="app"><main><button id="save">Save</button></main></div>`;
  sent = []; listeners = []; ticks = []; roots = []; clock = 0; global = {};
  timers = manual();
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
    tell({ t: "comment-mode", on: true });
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

  it("reports a same-document navigation", () => {
    sent = [];
    dispatchEvent(new HashChangeEvent("hashchange"));
    expect(sent).toEqual([{ t: "route", url: location.href }]);
  });

  it("sends a snapshot once the DOM has been quiet for a second while an address is pending, at most every 10 s", () => {
    tell(state([], { pending: true }));
    clock = 999;
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(0);
    clock = 20_000;
    tick(1000);
    const q = sent.filter(m => m.t === "quiet");
    expect(q).toHaveLength(1);
    expect(q[0].snapshot).toContain("Save");
    expect(q[0].snapshot).not.toContain("clax-overlay");
    clock = 25_000;
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(1);
    tell({ t: "snapshot-now" });
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(2);
    tell(state([], { pending: false }));
    clock = 60_000;
    tick(1000);
    expect(sent.filter(m => m.t === "quiet")).toHaveLength(2);
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
});
