// Waiting until the code under test has nothing left in hand: every request
// a stubbed `fetch` answered has been read, every lazy module has loaded,
// every macrotask hop it queued has run, and Svelte has applied the updates.
// A test then asserts on the page as it stands, a positive check without a
// poll and a negative one ("nothing was sent") with something to check.
//
// The page is idle after a pass in which nothing moved: no lazy import was
// loading; no zero-delay timer or `setImmediate` was waiting (the code's own
// `setTimeout(fn, 0)` hops, jsdom's `postMessage` deliveries and FileReader
// reads); no animation frame was requested; no response body was being read;
// no `fetch` mock was called or answered; and the document did not change.
// Work that waits on something else is not waited for: a request the test
// holds unanswered, a timer with a delay (advance the clock instead), a
// socket. Under fake timers the zero-delay timers and frames are the fake
// ones, run only by an advance, so they do not count either.
import { vi } from "vitest";
import { WAIT_MS } from "./timeouts";

type Tracker = {
  /** Zero-delay timers and immediates started through the globals and not yet run or cleared. */
  timers: Set<unknown>;
  /** Response body reads (`json`, `text`, …) in flight. */
  reads: Set<Promise<unknown>>;
  /** Animation frame callbacks requested through the global and not yet run or cancelled. */
  frames: Set<number>;
  /** Fetch results seen to settle. */
  answered: WeakSet<object>;
  answeredCount: number;
};

const KEY = Symbol.for("clax.test.settled");
const realNow = performance.now.bind(performance);

/** Starts tracking the work `settled` waits for. Idempotent: the tracker
 * lives on the global, so a module registry reset does not start a second. */
export function trackWork(): Tracker {
  const g = globalThis as { [KEY]?: Tracker };
  if (g[KEY]) return g[KEY];
  const t: Tracker = { timers: new Set(), frames: new Set(), reads: new Set(), answered: new WeakSet(), answeredCount: 0 };
  g[KEY] = t;
  const setT = globalThis.setTimeout;
  const clearT = globalThis.clearTimeout;
  globalThis.setTimeout = ((fn: TimerHandler, ms?: number, ...a: unknown[]) => {
    if (ms || typeof fn !== "function") return setT(fn as () => void, ms, ...a);
    const id = setT(() => { t.timers.delete(id); (fn as (...a: unknown[]) => void)(...a); }, 0);
    t.timers.add(id);
    return id;
  }) as typeof globalThis.setTimeout;
  globalThis.clearTimeout = ((id?: Parameters<typeof clearTimeout>[0]) => { t.timers.delete(id); clearT(id); }) as typeof globalThis.clearTimeout;
  // jsdom's FileReader reads on `setImmediate`.
  if (typeof setImmediate === "function") {
    const setI = globalThis.setImmediate;
    const clearI = globalThis.clearImmediate;
    globalThis.setImmediate = ((fn: (...a: unknown[]) => void, ...a: unknown[]) => {
      const id = setI(() => { t.timers.delete(id); fn(...a); });
      t.timers.add(id);
      return id;
    }) as typeof globalThis.setImmediate;
    globalThis.clearImmediate = ((id?: Parameters<typeof clearImmediate>[0]) => { t.timers.delete(id); clearI(id); }) as typeof globalThis.clearImmediate;
  }
  // jsdom runs animation frames on its own interval (`pretendToBeVisual`).
  if (typeof requestAnimationFrame === "function") {
    const raf = globalThis.requestAnimationFrame;
    const caf = globalThis.cancelAnimationFrame;
    globalThis.requestAnimationFrame = ((fn: FrameRequestCallback) => {
      const id = raf((at: number) => { t.frames.delete(id); fn(at); });
      t.frames.add(id);
      return id;
    }) as typeof globalThis.requestAnimationFrame;
    globalThis.cancelAnimationFrame = ((id: number) => { t.frames.delete(id); caf(id); }) as typeof globalThis.cancelAnimationFrame;
  }
  if (typeof Response !== "undefined") {
    for (const m of ["json", "text", "arrayBuffer", "blob", "formData", "bytes"] as const) {
      const proto = Response.prototype as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
      const read = proto[m];
      if (typeof read !== "function") continue;
      proto[m] = function (this: Response, ...a: unknown[]) {
        const p = read.apply(this, a);
        t.reads.add(p);
        const done = () => { t.reads.delete(p); };
        p.then(done, done);
        return p;
      };
    }
  }
  return t;
}

/** The fetch mock's calls and answers so far, as one reading that changes
 * whenever either does; "" when `fetch` is not a mock. */
function fetchReading(t: Tracker): string {
  const f = globalThis.fetch as unknown;
  if (!vi.isMockFunction(f)) return "";
  for (const r of f.mock.results) {
    const v = r.value as unknown;
    if (r.type !== "return" || !(v instanceof Promise) || t.answered.has(v)) continue;
    t.answered.add(v);
    const done = () => { t.answeredCount++; };
    v.then(done, done);
  }
  return `${f.mock.calls.length}/${t.answeredCount}`;
}

/** Resolves once the page has settled (see the top of this file). Throws,
 * naming what kept moving, if it has not within `WAIT_MS` of real time: a
 * guard for code that never goes idle (a zero-delay loop), not a budget. */
export async function settled(): Promise<void> {
  const t = trackWork();
  const { flushSync } = await import("svelte");
  const watch = typeof MutationObserver === "undefined" || typeof document === "undefined" ? null : new MutationObserver(() => {});
  watch?.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  const deadline = realNow() + WAIT_MS;
  try {
    for (;;) {
      const before = fetchReading(t);
      await Promise.allSettled([...t.reads]);
      // At least one macrotask (on vitest's own timers), and every module load.
      await vi.dynamicImportSettled();
      flushSync();
      const changed = (watch?.takeRecords().length ?? 0) > 0;
      const after = fetchReading(t);
      if (!changed && after === before && t.timers.size === 0 && t.frames.size === 0 && t.reads.size === 0) return;
      if (realNow() > deadline) {
        throw new Error(`the page never settled: ${[changed && "the document kept changing", after !== before && `fetch kept moving (${after})`, t.timers.size && `${t.timers.size} zero-delay timers or immediates waiting`, t.frames.size && `${t.frames.size} animation frames requested`, t.reads.size && `${t.reads.size} body reads in flight`].filter(Boolean).join(", ")}`);
      }
    }
  } finally {
    watch?.disconnect();
  }
}
