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
const snap = vi.hoisted(() => ({ throws: false }));
vi.mock("./snapshot", async orig => {
  const real = await orig<typeof import("./snapshot")>();
  return { ...real, serializeSnapshot: (doc: Document) => { if (snap.throws) throw new Error("hostile DOM"); return real.serializeSnapshot(doc); } };
});
const { COMPOSER_CONFIRM_MS, NOTICE_MS, startOnce } = await import("./overlay-app");
const { MAX_URL, URL_TOO_LONG } = await import("../messages");

type Listener = (m: unknown, sender: chrome.runtime.MessageSender) => void;
const WORKER = { id: "test-extension" } as chrome.runtime.MessageSender;
const PICK = "0123456789abcdef0123456789abcdef";
const PICK2 = "fedcba9876543210fedcba9876543210";
let nextId = PICK;

let sent: { t: string; [k: string]: unknown }[];
let listeners: Listener[];
let roots: ShadowRoot[];
let reply: (m: { t: string }) => unknown;
let stop: (() => void) | undefined;
let pending: { fn: () => void; ms: number }[];
const timers: Timers = {
  now: () => 0,
  set: (fn: () => void, ms: number) => { const e = { fn, ms }; pending.push(e); return e; },
  clear: h => { pending = pending.filter(e => e !== h); },
  frame: (fn: () => void) => { pending.push({ fn, ms: 0 }); },
};
/** Runs the animation frames and zero-delay timers due, and the promises they settle. */
async function run() {
  for (let i = 0; i < 10; i++) {
    const due = pending.filter(e => e.ms === 0);
    pending = pending.filter(e => e.ms !== 0);
    for (const e of due) e.fn();
    await new Promise(r => setTimeout(r, 0));
  }
}
/** Runs the timers of `ms`. */
const elapse = (ms: number) => { const due = pending.filter(e => e.ms === ms); pending = pending.filter(e => e.ms !== ms); due.forEach(e => e.fn()); };
const tell = (m: unknown) => listeners.forEach(l => l(m, WORKER));
const host = () => document.querySelector("clax-overlay") as HTMLElement;
const frames = () => roots.flatMap(r => [...r.querySelectorAll("iframe")]);
const save = () => document.getElementById("save")!;
const RECT = { x: 10, y: 20, w: 30, h: 40 };
/** A pick whose capture the worker answered and whose composer it opened. */
async function opened() {
  mode.handlers!.pickElement(save());
  await run();
  tell({ t: "open-composer", pickId: PICK, rect: RECT });
  return frames()[0];
}

beforeEach(() => {
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<main><button id="save">Save</button></main>`;
  sent = []; listeners = []; roots = []; pending = [];
  mode.on = false; mode.visible = true; mode.captured = 0;
  snap.throws = false;
  nextId = PICK;
  history.replaceState(null, "", "/app");
  reply = m => (m.t === "capture" ? { t: "captured", pickId: (m as { pickId?: string }).pickId ?? PICK, ok: true } : null);
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
    randomId: () => nextId,
  }, {});
  tell({ t: "state", page: null, route: null, threads: [], commentMode: true, pending: false });
});
afterEach(() => { stop?.(); vi.restoreAllMocks(); });

describe("a pick", () => {
  it("hides Clax's drawing for the capture and names the pick and its anchor", async () => {
    mode.handlers!.pickElement(save());
    expect(mode.visible).toBe(false);
    expect(host().style.visibility).toBe("hidden");
    await run();
    const capture = sent.find(m => m.t === "capture");
    expect(capture).toMatchObject({ t: "capture", pickId: PICK, dpr: 1, anchor: { kind: "element", quote: "Save" } });
    expect(mode.visible).toBe(true);
    expect(host().style.visibility).toBe("");
    expect(mode.captured).toBe(1);
    expect(frames()).toHaveLength(0);
  });

  it("keeps the composer's frame hidden and unfocused until the worker confirms its page, then serializes", async () => {
    const f = await opened();
    expect(f.src).toBe(`chrome-extension://test-extension/composer.html#${PICK}`);
    expect(mode.on).toBe(false);
    const focus = vi.spyOn(f, "focus");
    f.dispatchEvent(new Event("load"));
    await run();
    expect(f.style.visibility).toBe("hidden");
    expect(focus).not.toHaveBeenCalled();
    expect(sent.some(m => m.t === "pick")).toBe(false);
    tell({ t: "composer-ready", pickId: "f".repeat(32) });
    expect(f.style.visibility).toBe("hidden");
    tell({ t: "composer-ready", pickId: PICK });
    expect(f.style.visibility).toBe("");
    expect(focus).toHaveBeenCalledTimes(1);
    await run();
    const p = sent.find(m => m.t === "pick");
    expect(p).toMatchObject({ t: "pick", pickId: PICK, url: location.href, snapshotError: null });
    expect(p).not.toHaveProperty("anchor");
    expect(p?.snapshot).toContain("Save");
  });

  it("closes a frame whose page never connects, and tells the worker", async () => {
    const f = await opened();
    f.dispatchEvent(new Event("load"));
    elapse(COMPOSER_CONFIRM_MS);
    expect(frames()).toHaveLength(0);
    expect(sent.at(-1)).toEqual({ t: "cancel", pickId: PICK });
    tell({ t: "composer-ready", pickId: PICK });
    expect(frames()).toHaveLength(0);
  });

  it("closes the frame on a second load, before or after the confirmation", async () => {
    let f = await opened();
    f.dispatchEvent(new Event("load"));
    f.dispatchEvent(new Event("load"));
    expect(frames()).toHaveLength(0);
    expect(sent.at(-1)).toEqual({ t: "cancel", pickId: PICK });

    sent = [];
    f = await opened();
    tell({ t: "composer-ready", pickId: PICK });
    f.dispatchEvent(new Event("load")); // the composer's own load may come after it connected
    expect(frames()).toHaveLength(1);
    f.dispatchEvent(new Event("load"));
    expect(frames()).toHaveLength(0);
    expect(sent.at(-1)).toEqual({ t: "cancel", pickId: PICK });
  });

  it("opens the composer even when the worker's open-composer arrives before its capture answer", async () => {
    let answer!: (v: unknown) => void;
    reply = m => (m.t === "capture" ? new Promise(r => { answer = r; }) : null);
    mode.handlers!.pickElement(save());
    for (let i = 0; i < 2; i++) for (const e of pending.splice(0)) e.fn();
    await Promise.resolve();
    expect(sent.some(m => m.t === "capture")).toBe(true);
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(1);
    expect(host().style.visibility).toBe("");
    answer({ t: "captured", pickId: PICK, ok: true });
    await run();
    expect(frames()).toHaveLength(1);
  });

  it("ignores an open-composer for another pick or after the worker refused the capture", async () => {
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(0);
    reply = () => null;
    mode.handlers!.pickElement(save());
    await run();
    expect(mode.visible).toBe(true);
    tell({ t: "open-composer", pickId: PICK, rect: { x: 0, y: 0, w: 1, h: 1 } });
    tell({ t: "open-composer", pickId: "f".repeat(32), rect: { x: 0, y: 0, w: 1, h: 1 } });
    expect(frames()).toHaveLength(0);
  });

  it("closes the composer for its pick and turns comment mode back on after a post, as the worker's state has it", async () => {
    const state = (commentMode: boolean) => ({ t: "state", page: null, route: null, threads: [], commentMode, pending: false });
    tell(state(true));
    await opened();
    tell({ t: "composer-ready", pickId: PICK });
    await run();
    tell({ t: "close-composer", pickId: PICK, posted: true });
    expect(frames()).toHaveLength(0);
    expect(mode.on).toBe(true);
    // The side panel turned comment mode off while the composer was open: it stays off after the post.
    nextId = PICK2;
    mode.handlers!.pickElement(save());
    await run();
    tell({ t: "open-composer", pickId: PICK2, rect: RECT });
    tell(state(false));
    tell({ t: "close-composer", pickId: PICK2, posted: true });
    expect(frames()).toHaveLength(0);
    expect(mode.on).toBe(false);
  });

  it("says why when the worker closes a composer that never connected, for a while", async () => {
    await opened();
    tell({ t: "close-composer", pickId: PICK, posted: false, reason: "timeout" });
    expect(frames()).toHaveLength(0);
    const notice = () => roots.map(r => r.querySelector("[role=status]")?.textContent ?? "").join("");
    expect(notice()).toMatch(/comment box did not open/);
    elapse(NOTICE_MS);
    expect(notice()).toBe("");
  });

  it("gives comment mode back when the worker lost the pick, keeps the composer until a new pick replaces it", async () => {
    const f = await opened();
    tell({ t: "composer-ready", pickId: PICK });
    await run();
    const state = { t: "state", page: null, route: null, threads: [], commentMode: true, pending: false };
    tell(state);
    expect(mode.on).toBe(false); // the composer holds the page
    tell({ t: "pick-lost", pickId: "f".repeat(32) }); // another pick's: nothing
    expect(mode.on).toBe(false);
    tell({ t: "pick-lost", pickId: PICK });
    expect(mode.on).toBe(true);
    expect(frames()).toEqual([f]); // the person's text stays to copy
    tell(state);
    expect(mode.on).toBe(true);
    nextId = PICK2;
    mode.handlers!.pickElement(save());
    await run();
    tell({ t: "open-composer", pickId: PICK2, rect: RECT });
    expect(frames()).toHaveLength(1);
    expect(frames()[0].src).toContain(`#${PICK2}`);
  });

  it("closes a lost pick's composer when the worker relays the person's close, and comment mode stays", async () => {
    tell({ t: "state", page: null, route: null, threads: [], commentMode: true, pending: false });
    await opened();
    tell({ t: "composer-ready", pickId: PICK });
    await run();
    tell({ t: "pick-lost", pickId: PICK });
    tell({ t: "close-composer", pickId: PICK, posted: false });
    expect(frames()).toHaveLength(0);
    expect(mode.on).toBe(true);
  });

  it("refuses a pick, saying why, on a page whose address is too long for Clax", async () => {
    history.replaceState(null, "", `/app#${"x".repeat(MAX_URL)}`);
    mode.handlers!.pickElement(save());
    await run();
    expect(sent.filter(m => m.t === "capture")).toEqual([]);
    expect(mode.captured).toBe(1);
    expect(roots.map(r => r.querySelector("[role=status]")?.textContent ?? "").join("")).toBe(URL_TOO_LONG);
  });

  it("sends the pick with no address when the page's grew too long while the person typed", async () => {
    await opened();
    history.replaceState(null, "", `/app#${"x".repeat(MAX_URL)}`);
    tell({ t: "composer-ready", pickId: PICK });
    await run();
    expect(sent.find(m => m.t === "pick")).toMatchObject({ t: "pick", pickId: PICK, url: null });
  });

  it("still sends the pick, without a snapshot, when the serializer throws", async () => {
    snap.throws = true;
    await opened();
    tell({ t: "composer-ready", pickId: PICK });
    await run();
    expect(sent.find(m => m.t === "pick")).toMatchObject({ t: "pick", pickId: PICK, snapshot: null, snapshotError: "failed" });
  });
});
