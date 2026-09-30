import { describe, expect, it, vi } from "vitest";
import { NOT_TAKEN, PickFlow } from "../src/pick";
import type { Anchor, BridgeToShell } from "../src/protocol";

vi.setConfig({ testTimeout: 20_000 });

const anchor: Anchor = { kind: "element", selector: "h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const png = () => new Uint8Array([137, 80, 78, 71]).buffer;
const tick = () => new Promise(r => setTimeout(r, 0));

function flow(ms?: number) {
  const posted: BridgeToShell[] = [];
  let n = 0;
  const f = new PickFlow(window, m => { posted.push(m); }, 3, { ms, newId: () => `p${++n}` });
  return { f, posted, types: () => posted.map(m => m.type) };
}

describe("PickFlow", () => {
  it("posts the start with the anchor, and renders the clip only once the shell says the composer has focus", async () => {
    const { f, posted, types } = flow();
    const clip = vi.fn(async () => png());
    const done = f.start(anchor, clip);
    expect(posted[0]).toEqual({ type: "clax:pick-start", pickId: "p1", version: 3, anchor });
    await tick();
    expect(clip).not.toHaveBeenCalled();
    f.answer("p0", true);
    await tick();
    expect(clip).not.toHaveBeenCalled();
    f.answer("p1", true);
    await done;
    expect(clip).toHaveBeenCalledTimes(1);
    expect(types()).toEqual(["clax:pick-start", "clax:pick"]);
    expect(posted[1]).toMatchObject({ type: "clax:pick", pickId: "p1", anchor, clipError: undefined });
    expect((posted[1] as { clipPng?: ArrayBuffer }).clipPng?.byteLength).toBe(4);
    // A late second answer does nothing.
    f.answer("p1", true);
    await tick();
    expect(clip).toHaveBeenCalledTimes(1);
  });

  it("renders nothing for a refused start, and says no clip was taken", async () => {
    const { f, posted } = flow();
    const clip = vi.fn(async () => png());
    const done = f.start(anchor, clip);
    f.answer("p1", false);
    await done;
    expect(clip).not.toHaveBeenCalled();
    expect(posted[1]).toEqual({ type: "clax:pick", pickId: "p1", version: 3, anchor, clipError: NOT_TAKEN });
  });

  it("drops a pick with no answer in time: no render, and it says no clip was taken", async () => {
    const { f, posted } = flow(50);
    const clip = vi.fn(async () => png());
    await f.start(anchor, clip);
    expect(clip).not.toHaveBeenCalled();
    expect(posted[1]).toMatchObject({ type: "clax:pick", pickId: "p1", clipError: NOT_TAKEN });
  });

  it("waits 5 s for the answer by default (READY_MS)", async () => {
    vi.useFakeTimers();
    try {
      const { f, posted } = flow();
      const clip = vi.fn(async () => png());
      const done = f.start(anchor, clip);
      await vi.advanceTimersByTimeAsync(4_999);
      expect(posted).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(1);
      await done;
      expect(clip).not.toHaveBeenCalled();
      expect(posted[1]).toMatchObject({ clipError: NOT_TAKEN });
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops a pick still waiting when the next one starts", async () => {
    const { f, posted } = flow();
    const first = vi.fn(async () => png());
    const a = f.start(anchor, first);
    const b = f.start(anchor, async () => png());
    await a;
    expect(first).not.toHaveBeenCalled();
    expect(posted.map(m => `${m.type}:${(m as { pickId: string }).pickId}`)).toEqual(["clax:pick-start:p1", "clax:pick-start:p2", "clax:pick:p1"]);
    f.answer("p2", true);
    await b;
    expect(posted.at(-1)).toMatchObject({ type: "clax:pick", pickId: "p2", clipError: undefined });
  });

  it("keeps the timer functions in place when it was made, whatever the page puts there later", async () => {
    const { f, posted } = flow(50);
    const saved = { set: window.setTimeout, clear: window.clearTimeout };
    let done: Promise<void>;
    try {
      window.setTimeout = (() => { throw new Error("page"); }) as unknown as typeof setTimeout;
      window.clearTimeout = (() => { throw new Error("page"); }) as unknown as typeof clearTimeout;
      done = f.start(anchor, async () => png());
      // An answer clears the flow's own timer, not the page's.
      f.answer("p1", false);
      await done;
      done = f.start(anchor, async () => png());
    } finally {
      window.setTimeout = saved.set;
      window.clearTimeout = saved.clear;
    }
    // Its wait still ends on its own timer.
    await done;
    expect(posted.filter(m => m.type === "clax:pick").map(m => (m as { clipError?: string }).clipError)).toEqual([NOT_TAKEN, NOT_TAKEN]);
  });
});
