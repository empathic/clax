import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { type InspectTimers, MAX_BYTES, errorsOf, OUTLINE_BYTES, QUIET_MS, RING_SIZE, SETTLE_MS, SHRINKS, inspectPage, installErrorRing, outline, settle } from "../src/inspect";

/** Timers that run only when the test advances them. */
function manualTimers(): InspectTimers & { advance(ms: number): void; now: number } {
  let now = 0;
  let seq = 0;
  const due = new Map<number, { at: number; f: () => void }>();
  return {
    get now() { return now; },
    setTimeout: (f: () => void, ms: number) => { due.set(++seq, { at: now + ms, f }); return seq; },
    clearTimeout: (t: unknown) => { due.delete(t as number); },
    advance(ms: number) {
      const end = now + ms;
      for (;;) {
        const next = [...due.entries()].filter(([, d]) => d.at <= end).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
        if (!next) break;
        due.delete(next[0]);
        now = next[1].at;
        next[1].f();
      }
      now = end;
    },
  };
}

/** A MutationObserver the test fires by hand. */
class FakeObserver {
  static live: FakeObserver[] = [];
  observing = false;
  constructor(private readonly cb: () => void) { FakeObserver.live.push(this); }
  observe() { this.observing = true; }
  disconnect() { this.observing = false; }
  fire() { if (this.observing) this.cb(); }
}
const mutate = () => { for (const o of FakeObserver.live) o.fire(); };

/** A document whose readiness the test sets. */
const ready = (state: DocumentReadyState) => Object.defineProperty(document, "readyState", { value: state, configurable: true });

afterEach(() => {
  FakeObserver.live = [];
  delete (document as unknown as { readyState?: unknown }).readyState;
  document.body.innerHTML = "";
  document.title = "";
});

describe("the error ring", () => {
  // The ring's listeners stay on the test's window; each test reads its own ring.
  const before = console.error;
  afterEach(() => { console.error = before; });

  it("keeps the newest RING_SIZE errors in order, with their kinds, and still calls console.error", () => {
    const calls: unknown[][] = [];
    console.error = (...a: unknown[]) => { calls.push(a); };
    const ring = installErrorRing(window);
    console.error("first", { n: 1 });
    dispatchEvent(new ErrorEvent("error", { message: "boom", filename: "https://x.test/app.js", lineno: 3, colno: 7 }));
    const rejection = new Event("unhandledrejection") as Event & { reason?: unknown };
    rejection.reason = new TypeError("bad thing");
    dispatchEvent(rejection);
    expect(errorsOf(ring)).toEqual([
      { kind: "console", text: "first {\"n\":1}" },
      { kind: "error", text: "boom (https://x.test/app.js:3:7)" },
      { kind: "unhandledrejection", text: "TypeError: bad thing" },
    ]);
    expect(calls).toEqual([["first", { n: 1 }]]);
    for (let i = 0; i < RING_SIZE + 5; i++) console.error(`e${i}`);
    const kept = errorsOf(ring);
    expect(kept).toHaveLength(RING_SIZE);
    expect(kept[0].text).toBe("e5");
    expect(kept.at(-1)!.text).toBe(`e${RING_SIZE + 4}`);
  });

  it("lists resources that failed to load apart from the errors", () => {
    const ring = installErrorRing(window);
    document.body.innerHTML = `<img src="https://cdn.test/missing.png"><link rel="stylesheet" href="/gone.css">`;
    document.querySelector("img")!.dispatchEvent(new Event("error"));
    document.querySelector("link")!.dispatchEvent(new Event("error"));
    expect(ring.failed).toEqual(["https://cdn.test/missing.png", `${location.origin}/gone.css`]);
    expect(ring.errors).toEqual([]);
  });

  it("cuts a long error's text, and words what cannot be serialised", () => {
    console.error = () => {};
    const ring = installErrorRing(window);
    console.error("x".repeat(5000));
    const loop: Record<string, unknown> = {};
    loop.self = loop;
    console.error(loop);
    const [long, odd] = errorsOf(ring);
    expect(long.text.length).toBeLessThanOrEqual(501);
    expect(odd.text).toBe("[object Object]");
  });
});

describe("settle", () => {
  it("resolves once the DOM has been quiet for QUIET_MS", async () => {
    const t = manualTimers();
    let done = false;
    void settle(document, SETTLE_MS, { timers: t, Observer: FakeObserver as never }).then(() => { done = true; });
    t.advance(QUIET_MS - 100);
    mutate();
    t.advance(QUIET_MS - 1);
    await Promise.resolve();
    expect(done).toBe(false);
    t.advance(1);
    await Promise.resolve();
    expect(done).toBe(true);
    expect(t.now).toBe(QUIET_MS - 100 + QUIET_MS);
    expect(FakeObserver.live.every(o => !o.observing)).toBe(true);
  });

  it("resolves at settleMs at most while the DOM keeps changing", async () => {
    const t = manualTimers();
    let done = false;
    void settle(document, 1000, { timers: t, Observer: FakeObserver as never }).then(() => { done = true; });
    for (let i = 0; i < 9; i++) { t.advance(100); mutate(); }
    await Promise.resolve();
    expect(done).toBe(false);
    t.advance(100);
    await Promise.resolve();
    expect(done).toBe(true);
    expect(t.now).toBe(1000);
  });

  it("waits for the document's load once quiet, within settleMs", async () => {
    ready("interactive");
    const t = manualTimers();
    let done = false;
    void settle(document, SETTLE_MS, { timers: t, Observer: FakeObserver as never }).then(() => { done = true; });
    t.advance(QUIET_MS * 2);
    await Promise.resolve();
    expect(done).toBe(false);
    ready("complete");
    dispatchEvent(new Event("load"));
    await Promise.resolve();
    expect(done).toBe(true);
  });
});

describe("outline", () => {
  it("lists headings, buttons, labels, inputs with placeholders, and list lengths, skipping what is hidden", () => {
    document.body.innerHTML = `
      <h1>  Grocery   list </h1>
      <label for="q">Item</label><input id="q" placeholder="Add an item">
      <button>Add</button>
      <ul><li>Milk</li><li>Eggs</li><li>Bread</li></ul>
      <h2 hidden>Secret</h2>
      <div style="display: none"><button>Ghost</button></div>
      <select aria-label="Aisle"><option>1</option><option>2</option></select>
      <textarea placeholder="Notes"></textarea>
      <input type="submit" value="Save">
      <script>document.write("<h3>no</h3>")</script>`;
    expect(outline(document).split("\n")).toEqual([
      "# Grocery list",
      "label: Item",
      "[input] Add an item",
      "[button] Add",
      "list: 3 items",
      "[select] Aisle (2 options)",
      "[textarea] Notes",
      "[button] Save",
    ]);
  });

  it("stays within OUTLINE_BYTES", () => {
    document.body.innerHTML = Array.from({ length: 400 }, (_, i) => `<h2>Heading number ${i} with some words — ünïcödé</h2>`).join("");
    const text = outline(document);
    expect(new TextEncoder().encode(text).length).toBeLessThanOrEqual(OUTLINE_BYTES);
    expect(text.endsWith("…")).toBe(true);
  });
});

describe("inspectPage", () => {
  const ring = { errors: [{ kind: "console" as const, v: ["oops"] }], failed: ["https://cdn.test/a.png"] };
  const quick = () => {
    const t = manualTimers();
    return { t, deps: (render: (scale: number) => Promise<ArrayBuffer>) => ({ timers: t, Observer: FakeObserver as never, render }) };
  };
  /** Runs `p` while advancing the timers until it settles. */
  const run = async <T>(t: ReturnType<typeof manualTimers>, p: Promise<T>): Promise<T> => {
    const state = { settled: false };
    void p.then(() => { state.settled = true; }, () => { state.settled = true; });
    for (let i = 0; i < 100 && !state.settled; i++) { t.advance(QUIET_MS); await new Promise(r => setTimeout(r, 0)); }
    return p;
  };

  it("renders at scale 1 and returns the png, errors, failed loads, outline, title and height", async () => {
    document.title = "Groceries";
    document.body.innerHTML = "<h1>Groceries</h1>";
    const scales: number[] = [];
    const { t, deps } = quick();
    const r = await run(t, inspectPage(window, ring, {}, deps(async s => { scales.push(s); return new ArrayBuffer(10); })));
    expect(scales).toEqual([1]);
    expect(r).toMatchObject({ errors: [{ kind: "console", text: "oops" }], failed: ["https://cdn.test/a.png"], text: "# Groceries", title: "Groceries" });
    expect(r.png.byteLength).toBe(10);
    expect(typeof r.height).toBe("number");
  });

  it("renders again at half scale while the png is over maxBytes", async () => {
    const scales: number[] = [];
    const { t, deps } = quick();
    const r = await run(t, inspectPage(window, ring, { maxBytes: 100 }, deps(async s => { scales.push(s); return new ArrayBuffer(s > 0.25 ? 1000 : 50); })));
    expect(scales).toEqual([1, 0.5, 0.25]);
    expect(r.png.byteLength).toBe(50);
  });

  it("fails when the png is still over maxBytes after SHRINKS half-scale renders", async () => {
    const scales: number[] = [];
    const { t, deps } = quick();
    await expect(run(t, inspectPage(window, ring, { maxBytes: 100 }, deps(async s => { scales.push(s); return new ArrayBuffer(1000); })))).rejects.toThrow(/over 100 bytes/);
    expect(scales).toHaveLength(SHRINKS + 1);
  });

  it("takes MAX_BYTES and SETTLE_MS when not given, and bounds what it is given", async () => {
    expect(MAX_BYTES).toBe(1024 * 1024);
    expect(SETTLE_MS).toBe(3000);
    const { t, deps } = quick();
    const start = t.now;
    let mutating = true;
    const keepMutating = () => { if (mutating) { mutate(); t.setTimeout(keepMutating, 50); } };
    t.setTimeout(keepMutating, 50);
    await run(t, inspectPage(window, ring, { settleMs: 1e9 }, deps(async () => { mutating = false; return new ArrayBuffer(1); })));
    expect(t.now - start).toBeLessThanOrEqual(10_000 + QUIET_MS);
  });
});

describe("inspect.ts", () => {
  it("never waits on an animation frame (a hidden WebView runs none)", () => {
    const src = readFileSync(resolve(__dirname, "../src/inspect.ts"), "utf8");
    expect(src).not.toMatch(/requestAnimationFrame/);
  });
});
