// The home screen's time: the page's clock, and one for tests that moves
// only when told. Every timing rule of the page (the hold, the hover, the
// springs, the toss, the store's retries) runs on one of these.

/** The page's clock: `now()` in ms (monotonic), `date()` the wall time,
 * `after(ms, fn)` and `frame(fn)` (fn gets `now()`), each returning a
 * function that cancels it. */
export const realClock = Object.freeze({
  now: () => performance.now(),
  date: () => new Date(),
  after(ms, fn) {
    const t = setTimeout(fn, ms);
    return () => clearTimeout(t);
  },
  frame(fn) {
    const r = requestAnimationFrame(() => fn(performance.now()));
    return () => cancelAnimationFrame(r);
  },
});

/** A clock that moves only on `advance(ms)`, which runs the timers that
 * fall due in time order and a frame every 16 ms. `date()` starts at
 * `dateMs` and moves with it. */
export function manualClock(startMs = 0, dateMs = Date.UTC(2026, 9, 16, 12)) {
  let t = startMs;
  let seq = 0;
  const timers = new Map();
  let frames = new Map();
  let nextFrame = startMs + 16;
  return {
    now: () => t,
    date: () => new Date(dateMs + (t - startMs)),
    after(ms, fn) {
      const id = ++seq;
      timers.set(id, { at: t + Math.max(0, ms), fn });
      return () => timers.delete(id);
    },
    frame(fn) {
      const id = ++seq;
      frames.set(id, fn);
      return () => frames.delete(id);
    },
    advance(ms) {
      const end = t + ms;
      for (;;) {
        let due = null;
        for (const [id, x] of timers) if (x.at <= end && (!due || x.at < due[1].at)) due = [id, x];
        const at = Math.min(due ? due[1].at : Infinity, nextFrame);
        if (at > end) break;
        t = at;
        if (due && due[1].at === at) {
          timers.delete(due[0]);
          due[1].fn();
          continue;
        }
        nextFrame += 16;
        const run = frames;
        frames = new Map();
        for (const fn of run.values()) fn(t);
      }
      t = end;
    },
  };
}
