// The clock the shell's timing rules read: the shell-input window, the
// shield's double-click hole, consent arming, the publish budget, the hint,
// look marks, presence reports, a hidden tab's release, the wait for an
// opened thread's page and for a screenshot, and the bridge's part retries.
// In a production build it is the platform's clock and timers. A build made
// with the test clock (`CLAX_TEST_CLOCK=1`, the browser tests' build) adds a
// skew a test can advance through `globalThis.claxTestClock.advance(ms)`:
// both readings move forward by `ms`, and every timer started through
// `after` that falls due by then runs at once, in order. The skew lives in
// sessionStorage, so it holds across this tab's reloads as the budgets it
// governs do. A production build compiles `__CLAX_TEST_CLOCK__` to false, so
// none of that is in its bundle (web/scripts/bundle-size.mjs checks).

declare const __CLAX_TEST_CLOCK__: boolean;

const SKEW_KEY = "clax.test-clock.skew";
type Pending = { due: number; seq: number; fn: () => void; real: ReturnType<typeof setTimeout> };

let skew = 0;
let seq = 0;
const pending = new Set<Pending>();

if (__CLAX_TEST_CLOCK__) {
  try { skew = Number(sessionStorage.getItem(SKEW_KEY)) || 0; } catch { /* storage unavailable: no skew carried over */ }
  (globalThis as { claxTestClock?: unknown }).claxTestClock = {
    /** Moves the clock `ms` forward and runs every timer due by then. */
    advance(ms: number): number {
      const to = now() + ms;
      // Each due timer runs with the clock at its own time, so a timer it
      // starts falls due relative to that time, as it would in real time.
      for (;;) {
        let next: Pending | undefined;
        for (const p of pending) if (p.due <= to && (!next || p.due < next.due || (p.due === next.due && p.seq < next.seq))) next = p;
        if (!next) break;
        skew += Math.max(0, next.due - now());
        pending.delete(next);
        clearTimeout(next.real);
        next.fn();
      }
      skew += Math.max(0, to - now());
      try { sessionStorage.setItem(SKEW_KEY, String(skew)); } catch { /* storage unavailable: this load keeps it */ }
      return skew;
    },
    /** How far the clock has been moved, in milliseconds. */
    get skew() { return skew; },
    /** How many timers started through `after` are waiting. */
    get pending() { return pending.size; },
  };
}

/** Milliseconds on `performance.now()`'s scale. */
export const now = (): number => performance.now() + skew;

/** Milliseconds since the epoch, as `Date.now()`. */
export const wall = (): number => Date.now() + skew;

/** Runs `fn` once `ms` of this clock have passed; returns its cancel. */
export function after(ms: number, fn: () => void): () => void {
  if (!__CLAX_TEST_CLOCK__) {
    const id = setTimeout(fn, ms);
    return () => clearTimeout(id);
  }
  const p: Pending = { due: now() + ms, seq: ++seq, fn, real: setTimeout(() => { if (pending.delete(p)) fn(); }, ms) };
  pending.add(p);
  return () => { clearTimeout(p.real); pending.delete(p); };
}
