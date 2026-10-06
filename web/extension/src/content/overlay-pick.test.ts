// The overlay's side of a pick (spec 2026-10-05 §3.2, §8.1, §9.4). Comment
// mode acts only on trusted input, which jsdom cannot make, so CommentMode
// is replaced by a stand-in whose pick the test calls.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Timers } from "./resolver";

type Handlers = { pickElement(el: Element): void; cancel(): void };
const mode = vi.hoisted(() => ({ handlers: null as Handlers | null, on: false, visible: true, captured: 0 }));
vi.mock("../../../bridge/src/comment-mode", () => ({
  CommentMode: class {
    constructor(_doc: Document, h: Handlers) { mode.handlers = h; }
    set(on: boolean) { mode.on = on; }
    setVisible(on: boolean) { mode.visible = on; }
    captured() { mode.captured++; }
    destroy() {}
    flash() {}
  },
}));
const { startOnce } = await import("./overlay-app");

type Listener = (m: unknown, sender: chrome.runtime.MessageSender) => void;
const WORKER = { id: "test-extension" } as chrome.runtime.MessageSender;
const PICK = "0123456789abcdef0123456789abcdef";

let sent: { t: string; [k: string]: unknown }[];
let listeners: Listener[];
let roots: ShadowRoot[];
let reply: (m: { t: string }) => unknown;
let stop: (() => void) | undefined;
let pending: (() => void)[];
const timers: Timers = {
  now: () => 0,
  set: (fn: () => void) => { pending.push(fn); return fn; },
  clear: () => {},
  frame: (fn: () => void) => { pending.push(fn); },
};
/** Runs the timers and animation frames due, and the promises they settle. */
async function run() {
  for (let i = 0; i < 10; i++) {
    for (const fn of pending.splice(0)) fn();
    await new Promise(r => setTimeout(r, 0));
  }
}
const tell = (m: unknown) => listeners.forEach(l => l(m, WORKER));
const host = () => document.querySelector("clax-overlay") as HTMLElement;
const frames = () => roots.flatMap(r => [...r.querySelectorAll("iframe")]);

beforeEach(() => {
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<main><button id="save">Save</button></main>`;
  sent = []; listeners = []; roots = []; pending = [];
  mode.on = false; mode.visible = true; mode.captured = 0;
  reply = m => (m.t === "capture" ? { t: "captured", pickId: PICK, ok: true } : null);
  const attach = HTMLElement.prototype.attachShadow;
  vi.spyOn(HTMLElement.prototype, "attachShadow").mockImplementation(function (this: HTMLElement, init: ShadowRootInit) {
    const r = attach.call(this, init);
    roots.push(r);
    return r;
  });
  stop = startOnce({
    doc: document,
    runtime: {
      id: "test-extension",
      sendMessage: async m => { sent.push(m as never); return reply(m); },
      getURL: p => `chrome-extension://test-extension/${p}`,
      onMessage: { addListener: l => listeners.push(l), removeListener: l => { listeners = listeners.filter(x => x !== l); } },
    },
    timers,
    every: () => () => {},
    randomId: () => PICK,
  }, {});
  tell({ t: "state", page: null, route: null, threads: [], commentMode: true, pending: false });
});
afterEach(() => { stop?.(); vi.restoreAllMocks(); });

describe("a pick", () => {
  it("hides Clax's drawing for the capture, names the pick, and opens the composer when the worker says so", async () => {
    mode.handlers!.pickElement(document.getElementById("save")!);
    expect(mode.visible).toBe(false);
    expect(host().style.visibility).toBe("hidden");
    await run();
    const capture = sent.find(m => m.t === "capture");
    expect(capture).toMatchObject({ t: "capture", pickId: PICK, dpr: 1 });
    expect(mode.visible).toBe(true);
    expect(host().style.visibility).toBe("");
    expect(mode.captured).toBe(1);
    expect(frames()).toHaveLength(0);
    expect(sent.some(m => m.t === "pick")).toBe(false);

    tell({ t: "open-composer", pickId: PICK, rect: { x: 10, y: 20, w: 30, h: 40 } });
    expect(frames().map(f => f.src)).toEqual([`chrome-extension://test-extension/composer.html#${PICK}`]);
    expect(mode.on).toBe(false);
    await run();
    const p = sent.find(m => m.t === "pick");
    expect(p).toMatchObject({ t: "pick", pickId: PICK, url: location.href, snapshotError: null });
    expect(p?.anchor).toMatchObject({ kind: "element", quote: "Save" });
    expect(p?.snapshot).toContain("Save");
  });

  it("opens the composer even when the worker's open-composer arrives before its capture answer", async () => {
    let answer!: (v: unknown) => void;
    reply = m => (m.t === "capture" ? new Promise(r => { answer = r; }) : null);
    mode.handlers!.pickElement(document.getElementById("save")!);
    for (const fn of pending.splice(0)) fn();
    for (const fn of pending.splice(0)) fn();
    await Promise.resolve();
    expect(sent.some(m => m.t === "capture")).toBe(true);
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(1);
    expect(host().style.visibility).toBe("");
    answer({ t: "captured", pickId: PICK, ok: true });
    await run();
    expect(frames()).toHaveLength(1);
    expect(sent.filter(m => m.t === "pick")).toHaveLength(1);
  });

  it("ignores an open-composer for another pick or after the worker refused the capture", async () => {
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(0);
    reply = () => null;
    mode.handlers!.pickElement(document.getElementById("save")!);
    await run();
    expect(mode.visible).toBe(true);
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    tell({ t: "open-composer", pickId: "f".repeat(32), rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(0);
  });

  it("closes the composer for its pick and turns comment mode back on after a post", async () => {
    mode.handlers!.pickElement(document.getElementById("save")!);
    await run();
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    await run();
    tell({ t: "close-composer", pickId: PICK, posted: true });
    expect(frames()).toHaveLength(0);
    expect(mode.on).toBe(true);
  });

  it("closes a composer frame the page navigates, and tells the worker", async () => {
    mode.handlers!.pickElement(document.getElementById("save")!);
    await run();
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    const [f] = frames();
    const focus = vi.spyOn(f, "focus");
    f.dispatchEvent(new Event("load"));
    expect(focus).toHaveBeenCalledTimes(1);
    expect(frames()).toHaveLength(1);
    f.dispatchEvent(new Event("load"));
    expect(frames()).toHaveLength(0);
    expect(sent.at(-1)).toEqual({ t: "cancel", pickId: PICK });
  });
});
